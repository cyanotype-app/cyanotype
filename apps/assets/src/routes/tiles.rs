//! Basemap tiles for the map, served same-origin: `GET /assets/tiles/{name}` answers HTTP range requests out of a
//! PMTiles archive stored in the bucket (`basemap/{name}`, installed by `scripts/fetch-basemap.sh`).
//!
//! Why this exists: PMTiles is read by the browser with `Range` requests, and the object store (Linode) cannot be
//! given a CORS policy, so the browser can neither `fetch()` nor range-read bucket URLs. This relays only the bytes
//! asked for (a header, a directory, a tile: kilobytes), capped per request, so the 4 GB box never holds an archive.
//! Behind the session login like the rest of `/assets`, so an instance is not a public tile server.
//!
//! Only two fixed names are served, so the route cannot be used to read other objects from the bucket:
//! `world.pmtiles` (the low-zoom planet basemap) and `detail.pmtiles` (an optional higher-zoom regional extract).
use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use cyanotype_core::storage::RangeRead;
use serde::Serialize;

use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser};

/// The archives this route serves, with the bucket key of each.
const ARCHIVES: [(&str, &str); 2] = [
    ("world.pmtiles", "basemap/world.pmtiles"),
    ("detail.pmtiles", "basemap/detail.pmtiles"),
];

/// Longest range answered in one response (a PMTiles directory is at most ~16 KiB and a vector tile a few hundred
/// KiB). A larger request is answered with the first part, which HTTP allows.
const MAX_RANGE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Serialize)]
pub(super) struct Installed {
    /// A basemap is installed; the UI draws the plain-background fallback when false.
    world: bool,
    /// An optional higher-zoom regional extract is installed too.
    detail: bool,
}

/// `GET /assets/tiles`: which archives are installed.
pub(super) async fn installed(State(s): State<AppState>, _user: AuthUser) -> ApiResult<Json<Installed>> {
    let [world, detail] = ARCHIVES.map(|(_, key)| key);
    let (world, detail) = tokio::join!(s.storage.head_size(world), s.storage.head_size(detail));
    Ok(Json(Installed {
        world: world?.is_some_and(|n| n > 0),
        detail: detail?.is_some_and(|n| n > 0),
    }))
}

/// A parsed single `Range` header.
#[derive(Debug, PartialEq, Eq)]
enum Wanted {
    /// `bytes=first-last` (last inclusive) or `bytes=first-`.
    From { first: u64, last: Option<u64> },
    /// `bytes=-n`: the last n bytes.
    Suffix(u64),
}

fn parse_range(h: &str) -> Option<Wanted> {
    let spec = h.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None; // multipart ranges are not needed by PMTiles
    }
    let (a, b) = spec.split_once('-')?;
    match (a.trim(), b.trim()) {
        ("", "") => None,
        ("", n) => n.parse().ok().filter(|n| *n > 0).map(Wanted::Suffix),
        (f, "") => Some(Wanted::From {
            first: f.parse().ok()?,
            last: None,
        }),
        (f, l) => {
            let (first, last): (u64, u64) = (f.parse().ok()?, l.parse().ok()?);
            (last >= first).then_some(Wanted::From {
                first,
                last: Some(last),
            })
        }
    }
}

/// The range to ask the store for, with the length capped at [`MAX_RANGE_BYTES`].
fn store_range(w: &Wanted) -> String {
    match *w {
        Wanted::From { first, last } => {
            let cap = first.saturating_add(MAX_RANGE_BYTES - 1);
            format!("bytes={first}-{}", last.map_or(cap, |l| l.min(cap)))
        }
        Wanted::Suffix(n) => format!("bytes=-{}", n.min(MAX_RANGE_BYTES)),
    }
}

pub(super) async fn tile_range(
    State(s): State<AppState>,
    _user: AuthUser,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let key = ARCHIVES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, k)| *k)
        .ok_or(ApiError::NotFound)?;
    let wanted = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_range)
        .ok_or_else(|| ApiError::BadRequest("a single `Range: bytes=...` header is required".into()))?;
    let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok());
    match s.storage.get_range(key, &store_range(&wanted), if_match).await? {
        RangeRead::Found {
            body,
            content_range,
            etag,
        } => {
            let mut res = Response::new(Body::from(body));
            *res.status_mut() = StatusCode::PARTIAL_CONTENT;
            let h = res.headers_mut();
            h.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            // The browser may keep tiles for a day; the ETag makes a replaced archive visible on the next read.
            h.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, max-age=86400"),
            );
            if let Some(v) = content_range.and_then(|v| HeaderValue::from_str(&v).ok()) {
                h.insert(header::CONTENT_RANGE, v);
            }
            if let Some(v) = etag.and_then(|v| HeaderValue::from_str(&v).ok()) {
                h.insert(header::ETAG, v);
            }
            Ok(res)
        }
        RangeRead::NotFound => Err(ApiError::NotFound),
        RangeRead::Unsatisfiable => Ok(StatusCode::RANGE_NOT_SATISFIABLE.into_response()),
        RangeRead::PreconditionFailed => Ok(StatusCode::PRECONDITION_FAILED.into_response()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_ranges() {
        assert_eq!(
            parse_range("bytes=0-16383"),
            Some(Wanted::From {
                first: 0,
                last: Some(16383)
            })
        );
        assert_eq!(parse_range("bytes=100-"), Some(Wanted::From { first: 100, last: None }));
        assert_eq!(parse_range("bytes=-500"), Some(Wanted::Suffix(500)));
        for bad in [
            "",
            "bytes=",
            "bytes=-",
            "bytes=5-2",
            "bytes=0-1,5-6",
            "items=0-1",
            "bytes=a-b",
            "bytes=-0",
        ] {
            assert_eq!(parse_range(bad), None, "{bad}");
        }
    }

    #[test]
    fn long_ranges_are_capped() {
        let cap = MAX_RANGE_BYTES;
        assert_eq!(
            store_range(&Wanted::From {
                first: 10,
                last: Some(19)
            }),
            "bytes=10-19"
        );
        assert_eq!(
            store_range(&Wanted::From { first: 10, last: None }),
            format!("bytes=10-{}", 10 + cap - 1)
        );
        assert_eq!(
            store_range(&Wanted::From {
                first: 0,
                last: Some(u64::MAX)
            }),
            format!("bytes=0-{}", cap - 1)
        );
        assert_eq!(store_range(&Wanted::Suffix(u64::MAX)), format!("bytes=-{cap}"));
    }
}
