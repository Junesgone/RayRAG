//! Document assets: the thumbnail list and the image endpoint it points at.
//!
//! Upstream `document_api.py` answers `GET /api/v1/thumbnails?doc_ids=…` with a map from document id
//! to either a `data:` url or `/api/v1/documents/images/{kb_id}-{name}`, and serves that second url
//! from `GET /api/v1/documents/images/{image_id}`. RayRAG has no pre-rendered thumbnails, so it makes
//! them: an **image document** is decoded, scaled down and encoded as JPEG on first request, then
//! cached next to the other uploads. A document whose format cannot be decoded without a renderer (a
//! PDF, a Word file) has **no** thumbnail, and the map says `null` for it rather than inventing a
//! placeholder — the guide's own client treats `null` as "no thumbnail".
//!
//! The image id keeps upstream's `{kb_id}-{name}` shape, so a client that followed the documented
//! link does not have to know how the file is stored here.

use std::sync::Arc;

use axum::Extension;
use axum::Json;
use axum::extract::{Path, RawQuery, State};
use axum::response::{IntoResponse, Response};

use crate::server::{AppState, AuthContext, code};

/// Longest side of a generated thumbnail, in pixels.
const THUMBNAIL_MAX_SIDE: u32 = 200;
/// JPEG quality for the encoded thumbnail.
const THUMBNAIL_QUALITY: u8 = 80;

/// Extensions that can be decoded and therefore thumbnailed.
fn is_decodable_image(name: &str) -> bool {
    matches!(
        std::path::Path::new(name)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" | "tif" | "tiff"
    )
}

/// Where a document's own bytes live (the same layout the file endpoints use).
fn document_path(state: &AppState, doc: &crate::api::document::DocRecord) -> std::path::PathBuf {
    // `storage_name` is what the upload wrote and already carries the extension
    // (`{uuid}.{ext}`); appending it again would look for a file that never existed.
    let base = std::path::Path::new(&state.files.data_dir);
    let direct = base.join(&doc.storage_name);
    if direct.exists() {
        return direct;
    }
    let extension = std::path::Path::new(&doc.name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if extension.is_empty() {
        direct
    } else {
        base.join(format!("{}.{}", doc.storage_name, extension))
    }
}

/// The thumbnail cache directory.
fn thumbnail_dir(state: &AppState) -> std::path::PathBuf {
    std::path::Path::new(&state.files.data_dir).join("thumbnails")
}

fn thumbnail_path(state: &AppState, doc_id: &str) -> std::path::PathBuf {
    thumbnail_dir(state).join(format!("{doc_id}.jpg"))
}

/// Scale an image down to [`THUMBNAIL_MAX_SIDE`] and encode it as JPEG.
///
/// Aspect ratio is preserved, and an image already small enough is still re-encoded so the result is
/// always the same format (a client can therefore cache it by id).
pub(crate) fn make_thumbnail(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let image = image::load_from_memory(bytes)
        .map_err(|error| anyhow::anyhow!("could not decode the image: {error}"))?;
    let scaled = if image.width().max(image.height()) > THUMBNAIL_MAX_SIDE {
        image.thumbnail(THUMBNAIL_MAX_SIDE, THUMBNAIL_MAX_SIDE)
    } else {
        image
    };
    let mut out = Vec::new();
    let mut encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, THUMBNAIL_QUALITY);
    encoder
        .encode_image(&scaled.to_rgb8())
        .map_err(|error| anyhow::anyhow!("could not encode the thumbnail: {error}"))?;
    Ok(out)
}

/// The document's thumbnail, generating and caching it when it is missing.
///
/// `None` means "this document has no thumbnail" — a format that cannot be decoded, a missing file,
/// or a decode failure. It is never a placeholder.
pub(crate) fn ensure_thumbnail(
    state: &AppState,
    doc: &crate::api::document::DocRecord,
) -> Option<std::path::PathBuf> {
    let cached = thumbnail_path(state, &doc.id);
    if cached.exists() {
        return Some(cached);
    }
    if !is_decodable_image(&doc.name) {
        return None;
    }
    let source = document_path(state, doc);
    let bytes = std::fs::read(&source).ok()?;
    let thumbnail = make_thumbnail(&bytes).ok()?;
    std::fs::create_dir_all(thumbnail_dir(state)).ok()?;
    if std::fs::write(&cached, &thumbnail).is_err() {
        return None;
    }
    Some(cached)
}

/// The document a reader is allowed to see, or `None`.
fn visible_document(
    state: &AppState,
    auth: &AuthContext,
    doc_id: &str,
) -> Option<crate::api::document::DocRecord> {
    let doc = state.docs.get(doc_id)?;
    let kb = state.kbs.get(&doc.kb_id)?;
    if kb.owner_id == auth.user_id || auth.is_admin || kb.owner_id.is_empty() {
        Some(doc)
    } else {
        None
    }
}

/// `GET /api/v1/thumbnails?doc_ids=a&doc_ids=b`.
///
/// Upstream answers a missing list with HTTP 200 and business code 101, so the shape is kept: a
/// client checks the code, not the transport status.
pub async fn list_thumbnails(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    RawQuery(raw): RawQuery,
) -> Response {
    // The query is read raw rather than through a map extractor: `doc_ids` is **repeatable**, and a
    // `HashMap<String, String>` keeps only the last value — which silently dropped every document but
    // one. Some clients also send the ids comma-separated, so both spellings are accepted.
    let mut doc_ids: Vec<String> =
        url::form_urlencoded::parse(raw.as_deref().unwrap_or("").as_bytes())
            .filter(|(key, _)| key == "doc_ids" || key == "doc_ids[]")
            .flat_map(|(_, value)| {
                value
                    .split(',')
                    .map(|part| part.trim().to_string())
                    .collect::<Vec<String>>()
            })
            .filter(|value| !value.is_empty())
            .collect();
    let mut seen = std::collections::HashSet::new();
    doc_ids.retain(|value| seen.insert(value.clone()));
    if doc_ids.is_empty() {
        return Json(serde_json::json!({
            "code": code::INVALID_ARGUMENT,
            "message": "Lack of \"Document ID\"",
            "data": false,
        }))
        .into_response();
    }

    let mut data = serde_json::Map::new();
    for doc_id in doc_ids {
        match visible_document(&state, &auth, &doc_id) {
            Some(doc) => {
                let thumbnail = if ensure_thumbnail(&state, &doc).is_some() {
                    serde_json::json!(format!(
                        "/api/v1/documents/images/{}-{}.jpg",
                        doc.kb_id, doc.id
                    ))
                } else {
                    serde_json::Value::Null
                };
                data.insert(doc_id, thumbnail);
            }
            // A document the reader cannot see is reported exactly like one with no thumbnail.
            None => {
                data.insert(doc_id, serde_json::Value::Null);
            }
        }
    }
    Json(serde_json::json!({ "code": 0, "data": data })).into_response()
}

/// Split upstream's `{kb_id}-{name}` image id against the knowledge bases that exist.
///
/// Upstream's ids are 32 hex characters with no dashes, so it can split on the first one. RayRAG's
/// are dashed UUIDs, where that would cut the id in half — so the knowledge base is found by
/// **longest matching prefix** instead, and only the remainder is the name.
pub(crate) fn split_image_id<'a>(
    image_id: &'a str,
    kb_ids: &[String],
) -> Option<(String, &'a str)> {
    let mut best: Option<(String, &str)> = None;
    for kb_id in kb_ids {
        let prefix = format!("{kb_id}-");
        if let Some(name) = image_id.strip_prefix(&prefix) {
            if name.is_empty() {
                continue;
            }
            if best
                .as_ref()
                .is_none_or(|(current, _)| kb_id.len() > current.len())
            {
                best = Some((kb_id.clone(), name));
            }
        }
    }
    best
}

/// `GET /api/v1/documents/images/{image_id}`.
pub async fn document_image(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(image_id): Path<String>,
) -> Response {
    let not_found = || {
        crate::server::api_error_code(
            axum::http::StatusCode::NOT_FOUND,
            code::INVALID_OR_MISSING_DATA,
            "document not found",
        )
    };
    let kb_ids: Vec<String> = state.kbs.list().into_iter().map(|kb| kb.id).collect();
    let Some((kb_id, name)) = split_image_id(&image_id, &kb_ids) else {
        return not_found();
    };
    // The id is either a cached thumbnail (`{doc_id}.jpg`) or the document's own image file
    // (`{doc_id}.{ext}`); the document id is the name without its extension, so both resolve to the
    // same record — trimming only `.jpg` looked up `photo-1.png` for the original image.
    let doc_id = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let Some(doc) = visible_document(&state, &auth, doc_id) else {
        return not_found();
    };
    if doc.kb_id != kb_id {
        return not_found();
    }
    if name.ends_with(".jpg") {
        if let Some(path) = ensure_thumbnail(&state, &doc) {
            return crate::api::common::stream_stored_file(
                &path,
                "image/jpeg",
                "inline",
                "This thumbnail is empty.",
            )
            .await;
        }
        return not_found();
    }
    if !is_decodable_image(&doc.name) {
        return not_found();
    }
    let path = document_path(&state, &doc);
    if !path.exists() {
        return not_found();
    }
    let mime = crate::parser::mime_from_extension(&doc.name).unwrap_or("application/octet-stream");
    crate::api::common::stream_stored_file(&path, mime, "inline", "This image is empty.").await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut image = image::RgbImage::new(width, height);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]);
        }
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn a_thumbnail_is_jpeg_and_fits_the_longest_side() {
        let thumbnail = make_thumbnail(&png_bytes(800, 400)).unwrap();
        let decoded = image::load_from_memory(&thumbnail).unwrap();
        assert_eq!(decoded.width(), THUMBNAIL_MAX_SIDE);
        assert_eq!(decoded.height(), THUMBNAIL_MAX_SIDE / 2);
        // A JPEG starts with SOI (0xFFD8), which is what a browser will receive.
        assert_eq!(&thumbnail[..2], &[0xFF, 0xD8]);
    }

    #[test]
    fn a_small_image_keeps_its_size_and_still_becomes_a_thumbnail() {
        let thumbnail = make_thumbnail(&png_bytes(64, 32)).unwrap();
        let decoded = image::load_from_memory(&thumbnail).unwrap();
        assert_eq!(decoded.width(), 64);
        assert_eq!(decoded.height(), 32);
    }

    #[test]
    fn something_that_is_not_an_image_is_refused() {
        assert!(make_thumbnail(b"not an image").is_err());
    }

    #[test]
    fn the_documented_image_id_shape_is_parsed_around_dashed_uuids() {
        let kbs = vec![
            "4c09b0a3-ca29-424a-8ef0-43b80a6bf628".to_string(),
            "4c09b0a3-ca29-424a-8ef0-43b80a6bf629".to_string(),
        ];
        // The knowledge base is a dashed UUID, so the split cannot be at the first dash.
        assert_eq!(
            split_image_id("4c09b0a3-ca29-424a-8ef0-43b80a6bf628-photo-1.jpg", &kbs),
            Some((
                "4c09b0a3-ca29-424a-8ef0-43b80a6bf628".to_string(),
                "photo-1.jpg"
            ))
        );
        assert_eq!(
            split_image_id("4c09b0a3-ca29-424a-8ef0-43b80a6bf629-a-b.jpg", &kbs),
            Some((
                "4c09b0a3-ca29-424a-8ef0-43b80a6bf629".to_string(),
                "a-b.jpg"
            )),
            "a name may contain dashes"
        );
        assert_eq!(split_image_id("unknown-kb-photo.jpg", &kbs), None);
        assert_eq!(
            split_image_id("4c09b0a3-ca29-424a-8ef0-43b80a6bf628-", &kbs),
            None,
            "an id with no name is not resolvable"
        );
    }

    #[test]
    fn only_decodable_formats_are_offered_a_thumbnail() {
        for name in ["a.png", "b.JPG", "c.webp", "d.gif", "e.tiff"] {
            assert!(is_decodable_image(name), "{name}");
        }
        for name in ["a.pdf", "b.docx", "c.txt", "d.md", ""] {
            assert!(!is_decodable_image(name), "{name}");
        }
    }
}
