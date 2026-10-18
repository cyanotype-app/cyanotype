//! What the photo details panel shows, derived from the columns of `assets` and the EXIF dump the worker stored in
//! `assets.exif` (`apps/worker/src/exif.rs`): pure functions, no I/O, so the formatting rules are unit tested.
//!
//! The dump is a flat JSON object of tag name to a *display string*: `"FNumber": "1.8"`, `"ExposureTime": "1/120"`,
//! `"Flash": "fired, return light detected"`, `"GPSAltitudeRef": "above sea level"` (kamadak-exif's `display_as`, without
//! units), taken from the primary IFD with its Exif and GPS sub-IFDs. HEIC, JPEG and TIFF-based RAW all end up in that
//! shape (a RAW whose EXIF the EXIF reader cannot open keeps rawler's own serialisation instead: snake_case keys with
//! rationals as `{"n":..,"d":..}` and enums as numbers, which the readers below also accept). Because the panel is derived
//! from what is already stored, existing photos need no reprocessing.
use serde::Serialize;
use serde_json::Value;
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

/// What the date line of the panel needs.
#[derive(Serialize, Debug, PartialEq)]
pub struct DateInfo {
    /// The instant, RFC 3339 in UTC.
    pub instant: String,
    /// The wall-clock time of the photo, `YYYY-MM-DDTHH:MM:SS`, in its own zone when EXIF recorded it (`offset`), else
    /// the stored instant read as UTC (the only clock we have).
    pub local: String,
    /// The photo's UTC offset (`+02:00`) when EXIF has it; null when the time zone is unknown.
    pub offset: Option<String>,
    pub zone_known: bool,
    /// English weekday of `local`.
    pub weekday: String,
    /// Where the time comes from: `taken` (the camera's clock), `file` (the file's modification time) or `added` (when it
    /// was uploaded): the last two are the fallbacks the timeline uses, and the panel says so.
    pub basis: &'static str,
    /// The UTC calendar day the timeline files the photo under (`YYYY-MM-DD`).
    pub timeline_day: String,
}

#[derive(Serialize, Debug, PartialEq, Default)]
pub struct Exposure {
    /// The familiar one-liner: `ISO 100 · 26 mm · ƒ/1.8 · 1/120 s · +0.3 ev` (only the parts the photo has).
    pub line: Option<String>,
    pub iso: Option<String>,
    pub focal_length: Option<String>,
    /// The 35 mm equivalent, only when the EXIF has it and it says something the focal length does not.
    pub focal_length_35mm: Option<String>,
    pub aperture: Option<String>,
    pub shutter: Option<String>,
    pub bias: Option<String>,
    pub flash: Option<String>,
    pub white_balance: Option<String>,
    pub metering: Option<String>,
    pub program: Option<String>,
}

impl Exposure {
    pub fn is_empty(&self) -> bool {
        *self == Exposure::default()
    }
}

#[derive(Serialize, Debug, PartialEq, Default)]
pub struct GpsExtras {
    /// `123 m above sea level`.
    pub altitude: Option<String>,
    pub altitude_m: Option<f64>,
    /// `132° SE`, with ` (magnetic)` when the camera did not say true north.
    pub direction: Option<String>,
    pub direction_deg: Option<f64>,
}

#[derive(Serialize, Debug, PartialEq, Default)]
pub struct FileExtras {
    pub software: Option<String>,
    pub color_space: Option<String>,
}

/// `Tue`-style fields from the dump: everything the panel reads from `assets.exif`.
#[derive(Debug, PartialEq, Default)]
pub struct ExifDetails {
    pub exposure: Exposure,
    pub gps: GpsExtras,
    pub file: FileExtras,
}

// ---- reading the dump -------------------------------------------------------------------------------------------

fn field<'a>(dump: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| dump.get(k)).filter(|v| !v.is_null())
}

/// A non-empty string value (numbers are written out).
fn text(dump: &Value, keys: &[&str]) -> Option<String> {
    match field(dump, keys)? {
        Value::String(s) => {
            let s = s.replace('\0', "");
            let s = s.trim();
            (!s.is_empty()).then(|| s.to_string())
        }
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// A number: a JSON number, `{"n":18,"d":10}` (rawler), or the first value of a display string such as `1.8`, `100` or
/// `100, 100`. Anything else (`unknown`, `[reserved ...]`) is not a number.
fn number(dump: &Value, keys: &[&str]) -> Option<f64> {
    value_number(field(dump, keys)?)
}

fn value_number(v: &Value) -> Option<f64> {
    let n = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.split(',').next()?.trim().parse::<f64>().ok()?,
        Value::Object(o) => {
            let (n, d) = (o.get("n")?.as_f64()?, o.get("d")?.as_f64()?);
            if d == 0.0 {
                return None;
            }
            n / d
        }
        Value::Array(a) => return value_number(a.first()?),
        _ => return None,
    };
    n.is_finite().then_some(n)
}

/// The exposure time in seconds: `1/120`, `1/8.333333333333334`, `2`, `0.5` (display strings) or a rational.
fn exposure_seconds(dump: &Value) -> Option<f64> {
    let v = field(dump, &["ExposureTime", "exposure_time"])?;
    let secs = match v {
        Value::String(s) => match s.trim().split_once('/') {
            Some((a, b)) => {
                let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
                if b == 0.0 { return None } else { a / b }
            }
            None => s.trim().parse::<f64>().ok()?,
        },
        other => value_number(other)?,
    };
    (secs.is_finite() && secs > 0.0).then_some(secs)
}

// ---- formatting -----------------------------------------------------------------------------------------------

/// A number with at most `decimals` decimals and no trailing zeros: `26`, `5.7`, `1.8`.
pub fn trim_number(x: f64, decimals: usize) -> String {
    let s = format!("{x:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// `ƒ/1.8`: one decimal (an iPhone's 1.78 reads as the f/1.8 on the box), none for whole stops (`ƒ/4`, `ƒ/11`).
pub fn format_aperture(f: f64) -> Option<String> {
    (f > 0.0 && f < 1000.0).then(|| format!("ƒ/{}", trim_number(f, 1)))
}

/// `1/120 s` for a fast shutter, `0.5 s` / `2 s` / `1.3 s` from a third of a second up. Cameras write a slow exposure of
/// 0.3 s as `1/3.33..`, which as a fraction would read `1/3 s`: a decimal is what a photographer expects there.
pub fn format_shutter(secs: f64) -> Option<String> {
    if !(secs > 0.0 && secs.is_finite()) {
        return None;
    }
    Some(if secs < 0.3 {
        format!("1/{} s", (1.0 / secs).round() as u64)
    } else {
        format!("{} s", trim_number(secs, 1))
    })
}

/// `+0.3 ev`, `−0.7 ev`, `0 ev`: the sign is always written, the minus is a real minus sign, and thirds of a stop round to
/// one decimal.
pub fn format_bias(ev: f64) -> Option<String> {
    if !ev.is_finite() || ev.abs() > 30.0 {
        return None;
    }
    let r = (ev * 10.0).round() / 10.0;
    Some(if r == 0.0 {
        "0 ev".to_string()
    } else if r > 0.0 {
        format!("+{} ev", trim_number(r, 1))
    } else {
        format!("−{} ev", trim_number(-r, 1))
    })
}

pub fn format_focal(mm: f64) -> Option<String> {
    (mm > 0.0 && mm < 10_000.0).then(|| format!("{} mm", trim_number(mm, 1)))
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// A text value of an enumerated tag, unless it says nothing (`unknown`, `not defined`, `[reserved ...]`).
fn enum_text(dump: &Value, keys: &[&str], names: &[&str]) -> Option<String> {
    match field(dump, keys)? {
        Value::Number(n) => names
            .get(usize::try_from(n.as_u64()?).ok()?)
            .copied()
            .filter(|s| !s.is_empty())
            .map(capitalise),
        _ => {
            let s = text(dump, keys)?;
            let l = s.to_lowercase();
            (!l.starts_with('[') && l != "unknown" && l != "not defined" && l != "other").then(|| capitalise(&s))
        }
    }
}

const METERING: [&str; 7] = [
    "",
    "average",
    "center-weighted average",
    "spot",
    "multi-spot",
    "pattern",
    "partial",
];
const PROGRAMS: [&str; 9] = [
    "",
    "manual",
    "normal program",
    "aperture priority",
    "shutter priority",
    "creative program",
    "action program",
    "portrait mode",
    "landscape mode",
];

/// `Fired` / `Did not fire`, with the mode when the camera said it (`Fired, auto`, `Did not fire, suppressed`).
fn flash_label(dump: &Value) -> Option<String> {
    let v = field(dump, &["Flash", "flash"])?;
    let (fired, rest) = match v {
        Value::Number(n) => {
            let n = n.as_u64()?;
            let mode = match (n >> 3) & 3 {
                1 => ", forced",
                2 => ", suppressed",
                3 => ", auto",
                _ => "",
            };
            (n & 1 == 1, mode.to_string())
        }
        Value::String(s) => {
            let l = s.trim().to_lowercase();
            if l.starts_with('[') {
                return None;
            }
            let fired = l.starts_with("fired");
            if !fired && !l.starts_with("not fired") {
                return None;
            }
            let mode = ["forced", "suppressed", "auto"]
                .iter()
                .find(|m| l.split(", ").any(|p| p == **m))
                .map(|m| format!(", {m}"))
                .unwrap_or_default();
            (fired, mode)
        }
        _ => return None,
    };
    let red_eye = matches!(v, Value::String(s) if s.contains("red-eye"))
        .then_some(", red-eye reduction")
        .unwrap_or("");
    Some(format!(
        "{}{rest}{red_eye}",
        if fired { "Fired" } else { "Did not fire" }
    ))
}

fn white_balance(dump: &Value) -> Option<String> {
    match field(dump, &["WhiteBalance", "white_balance"])? {
        Value::Number(n) => match n.as_u64()? {
            0 => Some("Auto".into()),
            1 => Some("Manual".into()),
            _ => None,
        },
        Value::String(s) => {
            let l = s.trim().to_lowercase();
            if l.starts_with("auto") {
                Some("Auto".into())
            } else if l.starts_with("manual") {
                Some("Manual".into())
            } else {
                None
            }
        }
        _ => None,
    }
}

fn color_space(dump: &Value) -> Option<String> {
    match field(dump, &["ColorSpace", "color_space"])? {
        Value::Number(n) => match n.as_u64()? {
            1 => Some("sRGB".into()),
            0xffff => Some("Uncalibrated".into()),
            _ => None,
        },
        Value::String(s) => {
            let l = s.trim().to_lowercase();
            if l == "srgb" {
                Some("sRGB".into())
            } else if l == "uncalibrated" {
                Some("Uncalibrated".into())
            } else {
                None
            }
        }
        _ => None,
    }
}

const COMPASS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];

/// `132° SE` for a heading in degrees.
pub fn format_direction(deg: f64, magnetic: bool) -> Option<String> {
    if !(0.0..=360.0).contains(&deg) {
        return None;
    }
    let point = COMPASS[((deg / 45.0).round() as usize) % 8];
    Some(format!(
        "{}° {point}{}",
        deg.round() as i64 % 360,
        if magnetic { " (magnetic)" } else { "" }
    ))
}

/// Everything the panel reads from the EXIF dump apart from the date.
pub fn exif_details(dump: Option<&Value>) -> ExifDetails {
    let Some(d) = dump.filter(|d| d.is_object()) else {
        return ExifDetails::default();
    };

    let iso = number(
        d,
        &[
            "PhotographicSensitivity",
            "ISOSpeedRatings",
            "ISOSpeed",
            "iso_speed_ratings",
            "iso_speed",
        ],
    )
    .filter(|v| *v > 0.0 && *v < 10_000_000.0)
    .map(|v| format!("ISO {}", v.round() as u64));
    let focal = number(d, &["FocalLength", "focal_length"]).filter(|v| *v > 0.0);
    let focal_length = focal.and_then(format_focal);
    // The equivalent is worth a row only when it differs from the focal length (a full-frame camera repeats it); "0" and
    // "unknown" mean the camera did not know.
    let focal_length_35mm = number(d, &["FocalLengthIn35mmFilm"])
        .filter(|v| *v > 0.0)
        .filter(|v| focal.is_none_or(|f| (v - f).abs() >= 0.5))
        .and_then(format_focal);
    let aperture = number(d, &["FNumber", "fnumber"]).and_then(format_aperture);
    let shutter = exposure_seconds(d).and_then(format_shutter);
    let bias = number(d, &["ExposureBiasValue", "exposure_bias"]).and_then(format_bias);

    let line = [&iso, &focal_length, &aperture, &shutter, &bias]
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    let exposure = Exposure {
        line: (!line.is_empty()).then(|| line.join(" · ")),
        iso,
        focal_length,
        focal_length_35mm,
        aperture,
        shutter,
        bias,
        flash: flash_label(d),
        white_balance: white_balance(d),
        metering: enum_text(d, &["MeteringMode", "metering_mode"], &METERING),
        program: enum_text(d, &["ExposureProgram", "exposure_program"], &PROGRAMS),
    };

    // GPS: rawler nests these under "gps".
    let g = d.get("gps").filter(|g| g.is_object()).unwrap_or(d);
    let altitude_m = number(g, &["GPSAltitude", "gps_altitude"])
        .filter(|a| a.abs() < 100_000.0)
        .map(|a| {
            let below = match field(g, &["GPSAltitudeRef", "gps_altitude_ref"]) {
                Some(Value::Number(n)) => n.as_u64() == Some(1),
                Some(Value::String(s)) => s.to_lowercase().starts_with("below") || s.trim() == "1",
                _ => false,
            };
            if below { -a.abs() } else { a.abs() }
        });
    let altitude = altitude_m.map(|a| {
        let m = a.round() as i64;
        if m < 0 {
            format!("{} m below sea level", -m)
        } else {
            format!("{m} m above sea level")
        }
    });
    let direction_deg = number(g, &["GPSImgDirection", "gps_img_direction"]).filter(|v| (0.0..=360.0).contains(v));
    let magnetic = text(g, &["GPSImgDirectionRef", "gps_img_direction_ref"]).is_some_and(|s| {
        let l = s.to_lowercase();
        l.starts_with("magnetic") || l == "m"
    });
    let direction = direction_deg.and_then(|deg| format_direction(deg, magnetic));

    ExifDetails {
        exposure,
        gps: GpsExtras {
            altitude,
            altitude_m,
            direction,
            direction_deg,
        },
        file: FileExtras {
            software: text(d, &["Software"]).filter(|s| s.chars().count() <= 120),
            color_space: color_space(d),
        },
    }
}

// ---- the date ----------------------------------------------------------------------------------------------------

/// EXIF `YYYY:MM:DD HH:MM:SS` as a wall-clock time.
fn parse_wall_clock(s: &str) -> Option<PrimitiveDateTime> {
    let (d, t) = s.trim().split_once(' ')?;
    let mut dp = d.split(':').map(|x| x.parse::<i32>().ok());
    let (y, mo, da) = (dp.next()??, dp.next()??, dp.next()??);
    let mut tp = t.split(':').map(|x| x.trim().parse::<u8>().ok());
    let (h, mi, se) = (tp.next()??, tp.next()??, tp.next()??);
    let date =
        Date::from_calendar_date(y, Month::try_from(u8::try_from(mo).ok()?).ok()?, u8::try_from(da).ok()?).ok()?;
    Some(PrimitiveDateTime::new(date, Time::from_hms(h, mi, se).ok()?))
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

/// The UTC offset the camera recorded next to the capture time, if any. The worker keeps only the instant in
/// `taken_at` (a missing offset is read as UTC), so the offset comes from the dump: the first date tag that the worker
/// would have used, with its own `OffsetTime*` tag, and only when the two agree with the stored instant.
pub fn recorded_offset(dump: Option<&Value>, taken_at: Option<OffsetDateTime>) -> Option<UtcOffset> {
    let (d, taken) = (dump?, taken_at?);
    for (date, off) in [
        (
            ["DateTimeOriginal", "date_time_original"],
            ["OffsetTimeOriginal", "offset_time_original"],
        ),
        (
            ["DateTimeDigitized", "create_date"],
            ["OffsetTimeDigitized", "offset_time_digitized"],
        ),
        (["DateTime", "modify_date"], ["OffsetTime", "offset_time"]),
    ] {
        let Some(wall) = text(d, &date).and_then(|s| parse_wall_clock(&s)) else {
            continue;
        };
        // The first date tag that parses is the one `taken_at` came from.
        let off = text(d, &off).and_then(|s| parse_offset(&s))?;
        return (wall.assume_offset(off) == taken).then_some(off);
    }
    None
}

fn offset_text(o: UtcOffset) -> String {
    let (h, m, _) = o.as_hms();
    format!(
        "{}{:02}:{:02}",
        if o.is_negative() { '-' } else { '+' },
        h.abs(),
        m.abs()
    )
}

/// The date line for a photo: its own zone when EXIF has one.
pub fn date_info(
    taken_at: Option<OffsetDateTime>,
    file_modified_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    dump: Option<&Value>,
) -> DateInfo {
    date_info_in(taken_at, file_modified_at, created_at, dump, None)
}

/// [`date_info`] for a photo whose time may have been edited: `edited_zone` is the zone the edit chose (`Some(None)`: an
/// edited time whose zone is unknown; `Some(Some(secs))`: east of UTC by `secs`), None for a time nobody edited, which
/// reads its zone from the EXIF dump.
pub fn date_info_in(
    taken_at: Option<OffsetDateTime>,
    file_modified_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    dump: Option<&Value>,
    edited_zone: Option<Option<i32>>,
) -> DateInfo {
    let (instant, basis) = match (taken_at, file_modified_at) {
        (Some(t), _) => (t, "taken"),
        (None, Some(f)) => (f, "file"),
        (None, None) => (created_at, "added"),
    };
    let instant = instant.to_offset(UtcOffset::UTC);
    let offset = match edited_zone {
        Some(zone) => zone.and_then(|s| UtcOffset::from_whole_seconds(s).ok()),
        None if basis == "taken" => recorded_offset(dump, taken_at),
        None => None,
    };
    let local = instant.to_offset(offset.unwrap_or(UtcOffset::UTC));
    let fmt = time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]");
    DateInfo {
        instant: instant
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        local: local.format(&fmt).unwrap_or_default(),
        offset: offset.map(offset_text),
        zone_known: offset.is_some(),
        weekday: local.weekday().to_string(),
        basis,
        timeline_day: instant
            .format(&time::macros::format_description!("[year]-[month]-[day]"))
            .unwrap_or_default(),
    }
}

// ---- the file --------------------------------------------------------------------------------------------------

/// `JPEG`, `HEIC`, `CR3`... from the file name's extension, and whether it is a camera RAW.
pub fn format_of(file_ext: Option<&str>, content_type: &str) -> (String, bool) {
    let raw = file_ext.is_some_and(|e| cyanotype_core::pairing::RAW_EXTS.contains(&e));
    let label = match file_ext {
        Some("jpg" | "jpeg" | "jpe") => "JPEG".to_string(),
        Some("webp") => "WebP".to_string(),
        Some("tif" | "tiff") => "TIFF".to_string(),
        Some(e) if !e.is_empty() && e.len() <= 5 => e.to_uppercase(),
        _ => content_type
            .split('/')
            .nth(1)
            .map(|s| s.split(['+', ';']).next().unwrap_or(s).to_uppercase())
            .unwrap_or_else(|| content_type.to_string()),
    };
    (label, raw)
}

/// Megapixels as the spec sheets write them: `12 MP`, `8.3 MP`, `48 MP`.
pub fn megapixels(width: i32, height: i32) -> Option<String> {
    (width > 0 && height > 0).then(|| {
        let mp = f64::from(width) * f64::from(height) / 1e6;
        format!("{} MP", trim_number(mp, if mp < 10.0 { 1 } else { 0 }))
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use time::macros::datetime;

    use super::*;

    #[test]
    fn shutter_fractions() {
        assert_eq!(format_shutter(1.0 / 120.0).as_deref(), Some("1/120 s"));
        assert_eq!(format_shutter(1.0 / 8000.0).as_deref(), Some("1/8000 s"));
        // The camera's 1/119.99999 is the same shutter.
        assert_eq!(format_shutter(1.0 / 119.99999).as_deref(), Some("1/120 s"));
        assert_eq!(format_shutter(0.25).as_deref(), Some("1/4 s"));
        // A third of a second and slower reads as a decimal.
        assert_eq!(format_shutter(1.0 / 3.3333).as_deref(), Some("0.3 s"));
        assert_eq!(format_shutter(0.5).as_deref(), Some("0.5 s"));
        assert_eq!(format_shutter(2.0).as_deref(), Some("2 s"));
        assert_eq!(format_shutter(1.3).as_deref(), Some("1.3 s"));
        assert_eq!(format_shutter(30.0).as_deref(), Some("30 s"));
        assert_eq!(format_shutter(0.0), None);
        assert_eq!(format_shutter(f64::NAN), None);
    }

    #[test]
    fn f_number_rounding() {
        assert_eq!(format_aperture(1.8).as_deref(), Some("ƒ/1.8"));
        assert_eq!(format_aperture(1.78).as_deref(), Some("ƒ/1.8"));
        assert_eq!(format_aperture(4.0).as_deref(), Some("ƒ/4"));
        assert_eq!(format_aperture(2.8).as_deref(), Some("ƒ/2.8"));
        assert_eq!(format_aperture(11.0).as_deref(), Some("ƒ/11"));
        assert_eq!(format_aperture(0.0), None);
    }

    #[test]
    fn exposure_bias_sign() {
        assert_eq!(format_bias(0.3333333333333333).as_deref(), Some("+0.3 ev"));
        assert_eq!(format_bias(-0.6666666666666666).as_deref(), Some("−0.7 ev"));
        assert_eq!(format_bias(1.0).as_deref(), Some("+1 ev"));
        assert_eq!(format_bias(-2.0).as_deref(), Some("−2 ev"));
        assert_eq!(format_bias(0.0).as_deref(), Some("0 ev"));
        // Rounds to nothing: no sign on a zero.
        assert_eq!(format_bias(-0.04).as_deref(), Some("0 ev"));
    }

    fn dump() -> Value {
        json!({
            "Make": "Apple", "Model": "iPhone 15",
            "FNumber": "1.78", "ExposureTime": "1/119.99999", "PhotographicSensitivity": "100",
            "FocalLength": "6.765", "FocalLengthIn35mmFilm": "24", "ExposureBiasValue": "0.3333333333333333",
            "Flash": "not fired, auto", "WhiteBalance": "auto white balance", "MeteringMode": "pattern",
            "ExposureProgram": "normal program", "ColorSpace": "uncalibrated", "Software": "17.4.1",
            "GPSAltitude": "37.8", "GPSAltitudeRef": "above sea level",
            "GPSImgDirection": "132.4", "GPSImgDirectionRef": "true direction",
        })
    }

    #[test]
    fn the_familiar_exposure_line() {
        let e = exif_details(Some(&dump())).exposure;
        assert_eq!(e.line.as_deref(), Some("ISO 100 · 6.8 mm · ƒ/1.8 · 1/120 s · +0.3 ev"));
        assert_eq!(e.focal_length_35mm.as_deref(), Some("24 mm"));
        assert_eq!(e.flash.as_deref(), Some("Did not fire, auto"));
        assert_eq!(e.white_balance.as_deref(), Some("Auto"));
        assert_eq!(e.metering.as_deref(), Some("Pattern"));
        assert_eq!(e.program.as_deref(), Some("Normal program"));
    }

    #[test]
    fn the_35mm_equivalent_only_when_it_adds_something() {
        // A full-frame body repeats the focal length: no second row.
        let d = json!({"FocalLength": "50", "FocalLengthIn35mmFilm": "50"});
        let e = exif_details(Some(&d)).exposure;
        assert_eq!(e.focal_length.as_deref(), Some("50 mm"));
        assert_eq!(e.focal_length_35mm, None);
        // "0" means the camera does not know.
        let d = json!({"FocalLength": "23", "FocalLengthIn35mmFilm": "0"});
        assert_eq!(exif_details(Some(&d)).exposure.focal_length_35mm, None);
        // Without the focal length itself the equivalent is still shown.
        let d = json!({"FocalLengthIn35mmFilm": "35"});
        assert_eq!(
            exif_details(Some(&d)).exposure.focal_length_35mm.as_deref(),
            Some("35 mm")
        );
        // "unknown" (the dump's text for it) is not a number.
        let d = json!({"FocalLengthIn35mmFilm": "unknown"});
        assert_eq!(exif_details(Some(&d)).exposure.focal_length_35mm, None);
    }

    #[test]
    fn missing_fields_leave_rows_out() {
        assert_eq!(exif_details(None), ExifDetails::default());
        assert_eq!(exif_details(Some(&json!({}))), ExifDetails::default());
        assert_eq!(exif_details(Some(&json!("garbage"))), ExifDetails::default());
        // A partial exposure: the line holds only what is known, in order.
        let e = exif_details(Some(&json!({"FNumber": "2.8", "ExposureTime": "1/60"}))).exposure;
        assert_eq!(e.line.as_deref(), Some("ƒ/2.8 · 1/60 s"));
        assert!(e.iso.is_none() && e.bias.is_none() && e.flash.is_none());
        // Unreadable values are not rows.
        let d = json!({"MeteringMode": "[reserved metering mode 9]", "ExposureProgram": "not defined", "Flash": "[reserved]", "FNumber": "unknown"});
        assert!(exif_details(Some(&d)).exposure.is_empty());
    }

    #[test]
    fn exposure_time_forms() {
        let t = |s: &str| exif_details(Some(&json!({"ExposureTime": s}))).exposure.shutter;
        assert_eq!(t("1/250").as_deref(), Some("1/250 s"));
        assert_eq!(t("2").as_deref(), Some("2 s"));
        assert_eq!(t("0.5").as_deref(), Some("0.5 s"));
        assert_eq!(t("0/1"), None);
        assert_eq!(t("1/0"), None);
        assert_eq!(t("soon"), None);
    }

    #[test]
    fn gps_altitude_and_direction() {
        let g = exif_details(Some(&dump())).gps;
        assert_eq!(g.altitude.as_deref(), Some("38 m above sea level"));
        assert_eq!(g.altitude_m, Some(37.8));
        assert_eq!(g.direction.as_deref(), Some("132° SE"));
        let below = json!({"GPSAltitude": "12.4", "GPSAltitudeRef": "below sea level", "GPSImgDirection": "359.8", "GPSImgDirectionRef": "magnetic direction"});
        let g = exif_details(Some(&below)).gps;
        assert_eq!(g.altitude.as_deref(), Some("12 m below sea level"));
        assert_eq!(g.altitude_m, Some(-12.4));
        assert_eq!(g.direction.as_deref(), Some("0° N (magnetic)"));
        assert_eq!(
            exif_details(Some(&json!({"GPSImgDirection": "400"}))).gps.direction,
            None
        );
    }

    #[test]
    fn rawlers_own_dump_is_read_too() {
        let d = json!({
            "fnumber": {"n": 28, "d": 10}, "exposure_time": {"n": 1, "d": 500}, "iso_speed_ratings": 400,
            "focal_length": {"n": 35, "d": 1}, "exposure_bias": {"n": -2, "d": 3}, "flash": 9, "white_balance": 1,
            "metering_mode": 5, "exposure_program": 3, "color_space": 1,
            "gps": {"gps_altitude": {"n": 100, "d": 1}, "gps_altitude_ref": 1},
        });
        let r = exif_details(Some(&d));
        assert_eq!(
            r.exposure.line.as_deref(),
            Some("ISO 400 · 35 mm · ƒ/2.8 · 1/500 s · −0.7 ev")
        );
        assert_eq!(r.exposure.flash.as_deref(), Some("Fired, forced"));
        assert_eq!(r.exposure.white_balance.as_deref(), Some("Manual"));
        assert_eq!(r.exposure.metering.as_deref(), Some("Pattern"));
        assert_eq!(r.exposure.program.as_deref(), Some("Aperture priority"));
        assert_eq!(r.file.color_space.as_deref(), Some("sRGB"));
        assert_eq!(r.gps.altitude.as_deref(), Some("100 m below sea level"));
    }

    #[test]
    fn flash_wording() {
        let f = |s: &str| exif_details(Some(&json!({"Flash": s}))).exposure.flash;
        assert_eq!(f("fired").as_deref(), Some("Fired"));
        assert_eq!(f("fired, return light detected, auto").as_deref(), Some("Fired, auto"));
        assert_eq!(f("not fired, suppressed").as_deref(), Some("Did not fire, suppressed"));
        assert_eq!(f("not fired, no flash function").as_deref(), Some("Did not fire"));
        assert_eq!(
            f("fired, red-eye reduction").as_deref(),
            Some("Fired, red-eye reduction")
        );
    }

    #[test]
    fn colour_space_and_software() {
        let f = exif_details(Some(&json!({"ColorSpace": "sRGB", "Software": "  Adobe Lightroom  "}))).file;
        assert_eq!(f.color_space.as_deref(), Some("sRGB"));
        assert_eq!(f.software.as_deref(), Some("Adobe Lightroom"));
        let f = exif_details(Some(&json!({"ColorSpace": "[reserved color space 7]", "Software": ""}))).file;
        assert_eq!(f, FileExtras::default());
    }

    #[test]
    fn the_photos_own_zone_when_exif_has_it() {
        let d = json!({"DateTimeOriginal": "2023:07:04 13:05:09", "OffsetTimeOriginal": "+02:00"});
        let taken = datetime!(2023-07-04 11:05:09 UTC);
        let i = date_info(Some(taken), None, datetime!(2024-01-01 0:00 UTC), Some(&d));
        assert_eq!(i.local, "2023-07-04T13:05:09");
        assert_eq!(i.offset.as_deref(), Some("+02:00"));
        assert!(i.zone_known);
        assert_eq!(i.weekday, "Tuesday");
        assert_eq!(i.instant, "2023-07-04T11:05:09Z");
        assert_eq!(i.basis, "taken");
        assert_eq!(i.timeline_day, "2023-07-04");
    }

    #[test]
    fn the_weekday_and_day_follow_the_local_clock_but_the_timeline_follows_utc() {
        // 23:30 on Tuesday in Los Angeles is already Wednesday in UTC: the panel says Tuesday, the timeline's day is UTC's.
        let d = json!({"DateTimeOriginal": "2023:07:04 23:30:00", "OffsetTimeOriginal": "-07:00"});
        let i = date_info(
            Some(datetime!(2023-07-05 06:30:00 UTC)),
            None,
            datetime!(2024-01-01 0:00 UTC),
            Some(&d),
        );
        assert_eq!(i.weekday, "Tuesday");
        assert_eq!(i.offset.as_deref(), Some("-07:00"));
        assert_eq!(i.timeline_day, "2023-07-05");
    }

    #[test]
    fn an_edited_time_has_the_zone_the_edit_chose() {
        let d = json!({"DateTimeOriginal": "2023:07:04 13:05:09", "OffsetTimeOriginal": "+02:00"});
        let moved = datetime!(2023-07-05 02:30:00 UTC);
        let created = datetime!(2024-01-01 0:00 UTC);
        // The edit says UTC-07:00: the EXIF zone does not apply to the edited instant.
        let i = date_info_in(Some(moved), None, created, Some(&d), Some(Some(-7 * 3600)));
        assert_eq!(
            (i.local.as_str(), i.offset.as_deref(), i.zone_known),
            ("2023-07-04T19:30:00", Some("-07:00"), true)
        );
        assert_eq!(i.weekday, "Tuesday");
        // An edit that left the zone unknown reads as UTC, whatever the EXIF said.
        let i = date_info_in(Some(moved), None, created, Some(&d), Some(None));
        assert_eq!(
            (i.local.as_str(), i.offset, i.zone_known),
            ("2023-07-05T02:30:00", None, false)
        );
    }

    #[test]
    fn an_unknown_zone_is_said_so() {
        // No OffsetTime tag: the worker read the wall clock as UTC.
        let d = json!({"DateTimeOriginal": "2023:07:04 13:05:09"});
        let i = date_info(
            Some(datetime!(2023-07-04 13:05:09 UTC)),
            None,
            datetime!(2024-01-01 0:00 UTC),
            Some(&d),
        );
        assert_eq!(
            (i.local.as_str(), i.offset.clone(), i.zone_known),
            ("2023-07-04T13:05:09", None, false)
        );
        // No dump at all.
        let i = date_info(
            Some(datetime!(2023-07-04 13:05:09 UTC)),
            None,
            datetime!(2024-01-01 0:00 UTC),
            None,
        );
        assert!(!i.zone_known);
    }

    #[test]
    fn an_offset_that_disagrees_with_the_stored_instant_is_not_trusted() {
        // (A later change of taken_at, or an older worker.)
        let d = json!({"DateTimeOriginal": "2023:07:04 13:05:09", "OffsetTimeOriginal": "+02:00"});
        let i = date_info(
            Some(datetime!(2023-07-04 13:05:09 UTC)),
            None,
            datetime!(2024-01-01 0:00 UTC),
            Some(&d),
        );
        assert!(!i.zone_known);
    }

    #[test]
    fn without_a_capture_time_the_fallbacks_are_named() {
        let created = datetime!(2024-02-03 04:05:06 UTC);
        let i = date_info(None, Some(datetime!(2020-01-02 03:04:05 UTC)), created, None);
        assert_eq!((i.basis, i.local.as_str()), ("file", "2020-01-02T03:04:05"));
        let i = date_info(None, None, created, None);
        assert_eq!(
            (i.basis, i.local.as_str(), i.weekday.as_str()),
            ("added", "2024-02-03T04:05:06", "Saturday")
        );
    }

    #[test]
    fn file_formats() {
        assert_eq!(format_of(Some("jpg"), "image/jpeg"), ("JPEG".into(), false));
        assert_eq!(format_of(Some("heic"), "image/heic"), ("HEIC".into(), false));
        assert_eq!(format_of(Some("cr3"), "image/x-canon-cr3"), ("CR3".into(), true));
        assert_eq!(format_of(Some("webp"), "image/webp"), ("WebP".into(), false));
        assert_eq!(format_of(None, "image/png"), ("PNG".into(), false));
        assert_eq!(format_of(Some("verylongext"), "image/svg+xml"), ("SVG".into(), false));
    }

    #[test]
    fn megapixel_text() {
        assert_eq!(megapixels(4000, 3000).as_deref(), Some("12 MP"));
        assert_eq!(megapixels(3264, 2448).as_deref(), Some("8 MP"));
        assert_eq!(megapixels(8064, 6048).as_deref(), Some("49 MP"));
        assert_eq!(megapixels(2000, 1500).as_deref(), Some("3 MP"));
        assert_eq!(megapixels(1600, 1000).as_deref(), Some("1.6 MP"));
        assert_eq!(megapixels(0, 10), None);
    }
}
