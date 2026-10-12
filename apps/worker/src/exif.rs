//! EXIF extraction and mapping onto `assets` columns. Pure logic, no I/O.
use std::io::Cursor;

use exif::{In, Reader, Tag, Value};
use serde_json::{Map, Value as Json};
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

/// Fields we store on the asset row, plus the raw dump for the `exif` jsonb column.
#[derive(Debug, Default, PartialEq)]
pub struct ExifInfo {
    pub taken_at: Option<OffsetDateTime>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens_model: Option<String>,
    /// EXIF orientation 1..=8 (1 = upright).
    pub orientation: u8,
    pub json: Option<Json>,
}

/// Don't dump opaque blobs (MakerNote, thumbnails, ...) into jsonb.
const MAX_FIELD_BYTES: usize = 512;

pub fn parse(bytes: &[u8]) -> ExifInfo {
    let Ok(exif) = Reader::new().read_from_container(&mut Cursor::new(bytes)) else {
        return ExifInfo {
            orientation: 1,
            ..Default::default()
        };
    };

    let ascii = |tag: Tag| -> Option<String> {
        let f = exif.get_field(tag, In::PRIMARY)?;
        clean(&ascii_value(&f.value)?)
    };

    let taken_at = [Tag::DateTimeOriginal, Tag::DateTimeDigitized, Tag::DateTime]
        .into_iter()
        .find_map(|tag| {
            let dt = ascii(tag)?;
            // OffsetTime* tags are optional; without them we have no zone and assume UTC.
            let off = match tag {
                Tag::DateTimeOriginal => Tag::OffsetTimeOriginal,
                Tag::DateTimeDigitized => Tag::OffsetTimeDigitized,
                _ => Tag::OffsetTime,
            };
            parse_datetime(&dt, ascii(off).as_deref())
        });

    let gps = |tag: Tag, ref_tag: Tag, neg: u8| -> Option<f64> {
        let Value::Rational(r) = &exif.get_field(tag, In::PRIMARY)?.value else {
            return None;
        };
        let parts: Vec<(u32, u32)> = r.iter().map(|x| (x.num, x.denom)).collect();
        let deg = dms_to_degrees(&parts)?;
        let south_west = ascii(ref_tag).is_some_and(|s| s.as_bytes().first().map(u8::to_ascii_uppercase) == Some(neg));
        Some(if south_west { -deg } else { deg })
    };
    let lat = gps(Tag::GPSLatitude, Tag::GPSLatitudeRef, b'S').filter(|v| v.abs() <= 90.0);
    let lon = gps(Tag::GPSLongitude, Tag::GPSLongitudeRef, b'W').filter(|v| v.abs() <= 180.0);
    // Both or neither; and treat exactly (0, 0) as "no fix" (some cameras write zeros).
    let (latitude, longitude) = match (lat, lon) {
        (Some(a), Some(b)) if !(a == 0.0 && b == 0.0) => (Some(a), Some(b)),
        _ => (None, None),
    };

    let orientation = exif
        .get_field(Tag::Orientation, In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .filter(|o| (1..=8).contains(o))
        .unwrap_or(1) as u8;

    let mut map = Map::new();
    for f in exif.fields().filter(|f| f.ifd_num == In::PRIMARY) {
        let v = match &f.value {
            Value::Ascii(_) => ascii_value(&f.value).map(|s| Json::String(strip_nul(&s))),
            Value::Undefined(b, _) if b.len() > MAX_FIELD_BYTES => None,
            Value::Unknown(..) => None,
            v => Some(Json::String(strip_nul(&v.display_as(f.tag).to_string()))),
        };
        if let Some(v) = v {
            map.insert(f.tag.to_string(), v);
        }
    }

    ExifInfo {
        taken_at,
        latitude,
        longitude,
        camera_make: ascii(Tag::Make),
        camera_model: ascii(Tag::Model),
        lens_model: ascii(Tag::LensModel),
        orientation,
        json: (!map.is_empty()).then_some(Json::Object(map)),
    }
}

fn ascii_value(v: &Value) -> Option<String> {
    match v {
        Value::Ascii(parts) => Some(
            parts
                .iter()
                .map(|p| String::from_utf8_lossy(p).into_owned())
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    }
}

fn strip_nul(s: &str) -> String {
    // Postgres text/jsonb cannot hold NUL.
    s.replace('\0', "")
}

/// Trim whitespace/NULs; empty becomes None.
fn clean(s: &str) -> Option<String> {
    let s = strip_nul(s);
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// `(degrees, minutes, seconds)` rationals to decimal degrees.
pub fn dms_to_degrees(parts: &[(u32, u32)]) -> Option<f64> {
    if parts.len() < 3 || parts.iter().take(3).any(|&(_, d)| d == 0) {
        return None;
    }
    let f = |i: usize| f64::from(parts[i].0) / f64::from(parts[i].1);
    Some(f(0) + f(1) / 60.0 + f(2) / 3600.0)
}

/// EXIF "YYYY:MM:DD HH:MM:SS" with optional "+HH:MM" offset.
pub fn parse_datetime(s: &str, offset: Option<&str>) -> Option<OffsetDateTime> {
    let (d, t) = s.trim().split_once(' ')?;
    let mut dp = d.split(':').map(|x| x.parse::<i32>().ok());
    let (y, mo, da) = (dp.next()??, dp.next()??, dp.next()??);
    let mut tp = t.split(':').map(|x| x.trim().parse::<u8>().ok());
    let (h, mi, se) = (tp.next()??, tp.next()??, tp.next()??);
    let date =
        Date::from_calendar_date(y, Month::try_from(u8::try_from(mo).ok()?).ok()?, u8::try_from(da).ok()?).ok()?;
    let pdt = PrimitiveDateTime::new(date, Time::from_hms(h, mi, se).ok()?);
    let off = offset.and_then(parse_offset).unwrap_or(UtcOffset::UTC);
    Some(pdt.assume_offset(off))
}

fn parse_offset(s: &str) -> Option<UtcOffset> {
    let s = s.trim();
    let sign = match s.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let (h, m) = s[1..].split_once(':')?;
    UtcOffset::from_hms(sign * h.parse::<i8>().ok()?, sign * m.parse::<i8>().ok()?, 0).ok()
}

#[cfg(test)]
pub(crate) mod testutil {
    /// Minimal little-endian TIFF/EXIF block. `entries` are `(tag, type, count, value bytes)`
    /// where values longer than 4 bytes are placed after the IFD. Wrapped in a JPEG APP1.
    pub fn exif_app1(
        ifd0: &[(u16, u16, Vec<u8>)],
        exif_sub: &[(u16, u16, Vec<u8>)],
        gps: &[(u16, u16, Vec<u8>)],
    ) -> Vec<u8> {
        fn type_size(t: u16) -> usize {
            match t {
                2 => 1,
                3 => 2,
                5 => 8,
                _ => 1,
            }
        }
        // IFD layout: count(2) + n*12 + next(4), then out-of-line data.
        fn build_ifd(entries: &[(u16, u16, Vec<u8>)], base: u32, pointers: &[(u16, u32)]) -> Vec<u8> {
            let n = entries.len() + pointers.len();
            let mut data_off = base + 2 + 12 * n as u32 + 4;
            let mut head = Vec::new();
            let mut tail = Vec::new();
            head.extend((n as u16).to_le_bytes());
            let mut all: Vec<(u16, u16, u32, Vec<u8>)> = entries
                .iter()
                .map(|(tag, ty, v)| (*tag, *ty, (v.len() / type_size(*ty)) as u32, v.clone()))
                .collect();
            for (tag, ptr) in pointers {
                all.push((*tag, 4, 1, ptr.to_le_bytes().to_vec()));
            }
            all.sort_by_key(|e| e.0);
            for (tag, ty, count, v) in all {
                head.extend(tag.to_le_bytes());
                head.extend(ty.to_le_bytes());
                head.extend(count.to_le_bytes());
                if v.len() <= 4 {
                    let mut p = v.clone();
                    p.resize(4, 0);
                    head.extend(p);
                } else {
                    head.extend(data_off.to_le_bytes());
                    data_off += v.len() as u32;
                    tail.extend(v);
                }
            }
            head.extend(0u32.to_le_bytes());
            head.extend(tail);
            head
        }
        // Lengths don't depend on pointer values, so size them first, then lay out
        // IFD0, Exif sub-IFD, GPS IFD back to back.
        let ptrs = |e: u32, g: u32| {
            let mut v = Vec::new();
            if !exif_sub.is_empty() {
                v.push((0x8769, e));
            }
            if !gps.is_empty() {
                v.push((0x8825, g));
            }
            v
        };
        let ifd0_len = build_ifd(ifd0, 8, &ptrs(0, 0)).len() as u32;
        let exif_off = 8 + ifd0_len;
        let exif_len = if exif_sub.is_empty() {
            0
        } else {
            build_ifd(exif_sub, exif_off, &[]).len() as u32
        };
        let gps_off = exif_off + exif_len;
        let mut tiff = b"II*\0".to_vec();
        tiff.extend(8u32.to_le_bytes());
        tiff.extend(build_ifd(ifd0, 8, &ptrs(exif_off, gps_off)));
        if !exif_sub.is_empty() {
            tiff.extend(build_ifd(exif_sub, exif_off, &[]));
        }
        if !gps.is_empty() {
            tiff.extend(build_ifd(gps, gps_off, &[]));
        }
        let mut seg = b"Exif\0\0".to_vec();
        seg.extend(tiff);
        let mut out = vec![0xFF, 0xE1];
        out.extend((seg.len() as u16 + 2).to_be_bytes());
        out.extend(seg);
        out
    }

    pub fn ascii(s: &str) -> Vec<u8> {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        v
    }

    pub fn rationals(v: &[(u32, u32)]) -> Vec<u8> {
        v.iter()
            .flat_map(|(n, d)| n.to_le_bytes().into_iter().chain(d.to_le_bytes()))
            .collect()
    }

    /// Insert an APP1 segment right after the SOI marker of a JPEG.
    pub fn with_app1(jpeg: &[u8], app1: &[u8]) -> Vec<u8> {
        let mut out = jpeg[..2].to_vec();
        out.extend(app1);
        out.extend(&jpeg[2..]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    #[test]
    fn datetime_parsing() {
        let t = parse_datetime("2023:07:04 13:05:09", Some("+02:00")).unwrap();
        assert_eq!(Some(t), parse_datetime("2023:07:04 11:05:09", None));
        assert_eq!(
            parse_datetime("2023:07:04 13:05:09", None).unwrap().offset(),
            UtcOffset::UTC
        );
        assert!(parse_datetime("0000:00:00 00:00:00", None).is_none());
        assert!(parse_datetime("garbage", None).is_none());
    }

    #[test]
    fn dms() {
        let d = dms_to_degrees(&[(37, 1), (30, 1), (0, 1)]).unwrap();
        assert!((d - 37.5).abs() < 1e-9);
        assert!(dms_to_degrees(&[(1, 0), (0, 1), (0, 1)]).is_none());
        assert!(dms_to_degrees(&[(1, 1)]).is_none());
    }

    #[test]
    fn no_exif_gives_defaults() {
        let i = parse(b"not an image");
        assert_eq!(i.orientation, 1);
        assert!(i.json.is_none() && i.taken_at.is_none());
    }

    #[test]
    fn full_mapping_from_generated_jpeg() {
        let ifd0 = vec![
            (0x010F, 2, ascii("ACME")),
            (0x0110, 2, ascii("Shooter 9")),
            (0x0112, 3, 6u16.to_le_bytes().to_vec()),
        ];
        let sub = vec![
            (0x9003, 2, ascii("2021:12:31 23:59:58")),
            (0x9011, 2, ascii("-05:00")),
            (0xA434, 2, ascii("ACME 24mm f/2")),
        ];
        let gps = vec![
            (0x0001, 2, ascii("S")),
            (0x0002, 5, rationals(&[(33, 1), (51, 1), (0, 1)])),
            (0x0003, 2, ascii("W")),
            (0x0004, 5, rationals(&[(151, 1), (12, 1), (36, 1)])),
        ];
        let jpeg = with_app1(&tiny_jpeg(), &exif_app1(&ifd0, &sub, &gps));
        let i = parse(&jpeg);
        assert_eq!(i.camera_make.as_deref(), Some("ACME"));
        assert_eq!(i.camera_model.as_deref(), Some("Shooter 9"));
        assert_eq!(i.orientation, 6);
        assert_eq!(i.taken_at, parse_datetime("2021:12:31 23:59:58", Some("-05:00")));
        assert_eq!(i.taken_at.unwrap().offset().whole_hours(), -5);
        assert_eq!(i.lens_model.as_deref(), Some("ACME 24mm f/2"));
        assert!((i.latitude.unwrap() + 33.85).abs() < 1e-9);
        assert!((i.longitude.unwrap() + 151.21).abs() < 1e-9);
        let j = i.json.unwrap();
        assert_eq!(j["Make"], "ACME");
        assert!(j.get("GPSLatitude").is_some());
    }

    #[test]
    fn zero_gps_is_ignored() {
        let gps = vec![
            (0x0001, 2, ascii("N")),
            (0x0002, 5, rationals(&[(0, 1), (0, 1), (0, 1)])),
            (0x0003, 2, ascii("E")),
            (0x0004, 5, rationals(&[(0, 1), (0, 1), (0, 1)])),
        ];
        let jpeg = with_app1(&tiny_jpeg(), &exif_app1(&[(0x010F, 2, ascii("A"))], &[], &gps));
        let i = parse(&jpeg);
        assert_eq!((i.latitude, i.longitude), (None, None));
    }

    pub fn tiny_jpeg() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(8, 4, image::Rgb([200, 30, 30]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Jpeg)
            .unwrap();
        out
    }
}
