//! What a Takeout JSON sidecar says about its photo. Only the fields Cyanotype uses are read; everything else (view counts,
//! the sharing URL, people) is ignored, and a field of an unexpected shape is treated as absent, never as an error.
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The longest sidecar worth reading (real ones are 1 to 3 KB; a huge one is not a sidecar).
pub const MAX_SIDECAR_BYTES: usize = 64 * 1024;
/// The longest caption kept (the library's own limit).
pub const MAX_DESCRIPTION_CHARS: usize = 2000;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    /// `photoTakenTime.timestamp`: when the photo was taken, seconds since the epoch (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taken_at: Option<i64>,
    /// `geoData` (what Google Photos shows, so including a place the owner set there), else `geoDataExif` (what the file
    /// says). 0, 0 means "no location" in Takeout and is dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latitude: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub longitude: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub favorited: bool,
    /// Google Photos' archive: becomes Cyanotype's archive.
    #[serde(default, skip_serializing_if = "is_false")]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub trashed: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A JSON number, or a string holding one (Takeout writes timestamps as strings).
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn geo(v: Option<&Value>) -> Option<(f64, f64)> {
    let v = v?;
    let (lat, lon) = (number(v.get("latitude")?)?, number(v.get("longitude")?)?);
    // Takeout writes 0, 0 for "no location"; an out-of-range value is garbage.
    if !lat.is_finite() || !lon.is_finite() || (lat == 0.0 && lon == 0.0) || lat.abs() > 90.0 || lon.abs() > 180.0 {
        return None;
    }
    Some((lat, lon))
}

/// Read a sidecar. None when it is not JSON, not an object, or too large.
pub fn parse(json: &str) -> Option<Meta> {
    if json.len() > MAX_SIDECAR_BYTES {
        return None;
    }
    let v: Value = serde_json::from_str(json).ok()?;
    let o = v.as_object()?;
    let taken_at = o
        .get("photoTakenTime")
        .and_then(|t| t.get("timestamp"))
        .and_then(number)
        .filter(|t| t.is_finite() && *t > 0.0)
        .map(|t| t as i64);
    let (latitude, longitude) = match geo(o.get("geoData")).or_else(|| geo(o.get("geoDataExif"))) {
        Some((la, lo)) => (Some(la), Some(lo)),
        None => (None, None),
    };
    let description = o
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|d| !d.is_empty() && !d.contains('\0'))
        .map(|d| d.chars().take(MAX_DESCRIPTION_CHARS).collect());
    let flag = |k: &str| o.get(k).and_then(Value::as_bool).unwrap_or(false);
    Some(Meta {
        taken_at,
        latitude,
        longitude,
        description,
        favorited: flag("favorited"),
        archived: flag("archived"),
        trashed: flag("trashed"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_typical_sidecar() {
        let m = parse(
            r#"{"title":"IMG_1234.JPG","description":"  Dinner by the river ","imageViews":"3",
                "creationTime":{"timestamp":"1700000000","formatted":"x"},
                "photoTakenTime":{"timestamp":"1561896000","formatted":"Jun 30, 2019"},
                "geoData":{"latitude":38.7223,"longitude":-9.1393,"altitude":12.0,"latitudeSpan":0.0,"longitudeSpan":0.0},
                "geoDataExif":{"latitude":0.0,"longitude":0.0},
                "favorited":true,"archived":false,"url":"https://photos.google.com/photo/x"}"#,
        )
        .unwrap();
        assert_eq!(m.taken_at, Some(1_561_896_000));
        assert_eq!((m.latitude, m.longitude), (Some(38.7223), Some(-9.1393)));
        assert_eq!(m.description.as_deref(), Some("Dinner by the river"));
        assert!(m.favorited && !m.archived && !m.trashed);
    }

    #[test]
    fn no_location_is_zero_zero_and_exif_is_the_fallback() {
        let none =
            parse(r#"{"geoData":{"latitude":0.0,"longitude":0.0},"geoDataExif":{"latitude":0.0,"longitude":0.0}}"#)
                .unwrap();
        assert_eq!((none.latitude, none.longitude), (None, None));
        let exif = parse(
            r#"{"geoData":{"latitude":0.0,"longitude":0.0},"geoDataExif":{"latitude":-33.86,"longitude":151.2}}"#,
        )
        .unwrap();
        assert_eq!((exif.latitude, exif.longitude), (Some(-33.86), Some(151.2)));
        // What the owner set in Google Photos wins over the file's own value.
        let both = parse(
            r#"{"geoData":{"latitude":1.5,"longitude":2.5},"geoDataExif":{"latitude":-33.86,"longitude":151.2}}"#,
        )
        .unwrap();
        assert_eq!((both.latitude, both.longitude), (Some(1.5), Some(2.5)));
    }

    #[test]
    fn odd_values_are_absent_not_errors() {
        let m = parse(
            r#"{"photoTakenTime":{"timestamp":"soon"},"geoData":{"latitude":95.0,"longitude":10.0},"description":"   ","favorited":"yes"}"#,
        )
        .unwrap();
        assert_eq!(m, Meta::default());
        let m = parse(r#"{"photoTakenTime":{"timestamp":1561896000}}"#).unwrap();
        assert_eq!(m.taken_at, Some(1_561_896_000), "a number works as well as a string");
        assert_eq!(parse(r#"{"photoTakenTime":{"timestamp":"0"}}"#).unwrap().taken_at, None);
        assert!(parse("[]").is_none() && parse("not json").is_none() && parse("").is_none());
        assert!(parse(&format!(r#"{{"description":"{}"}}"#, "x".repeat(MAX_SIDECAR_BYTES))).is_none());
    }

    #[test]
    fn a_long_description_is_cut_to_the_library_limit() {
        let m = parse(&format!(
            r#"{{"description":"{}"}}"#,
            "é".repeat(MAX_DESCRIPTION_CHARS + 50)
        ))
        .unwrap();
        assert_eq!(m.description.unwrap().chars().count(), MAX_DESCRIPTION_CHARS);
    }

    #[test]
    fn archived_and_trashed_are_kept() {
        let m = parse(r#"{"archived":true,"trashed":true}"#).unwrap();
        assert!(m.archived && m.trashed);
    }
}
