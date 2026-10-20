//! The `prism://` scheme the frontend loads images from.
//!
//! `prism://localhost/cover/<coverId>/<size>` (on Windows the WebView sees
//! `http://prism.localhost/cover/<coverId>/<size>`) returns the cover resized to a square
//! bound of 96, 300 or 600 px as JPEG. Resized covers are cached on disk.

use std::io::Cursor;
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{Manager, Runtime, UriSchemeContext, UriSchemeResponder};

use crate::library::covers::CoverStore;
use crate::AppState;

const SIZES: [u32; 3] = [96, 300, 600];
const JPEG_QUALITY: u8 = 85;

pub fn register<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    let path = request.uri().path().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let store = &app.state::<AppState>().library.covers;
        let response = match store.dir().parent() {
            Some(cache) => handle(store, cache, &path),
            None => plain(StatusCode::INTERNAL_SERVER_ERROR, "no cache dir"),
        };
        responder.respond(response);
    });
}

/// Serves `/cover/<id>/<size>`. `cache` is the directory holding one subfolder per size.
pub fn handle(store: &CoverStore, cache: &Path, path: &str) -> Response<Vec<u8>> {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let [kind, id, size] = parts.as_slice() else {
        return plain(StatusCode::BAD_REQUEST, "malformed path");
    };
    if *kind != "cover" {
        return plain(StatusCode::BAD_REQUEST, "malformed path");
    }
    let Some(size) = size.parse::<u32>().ok().filter(|s| SIZES.contains(s)) else {
        return plain(StatusCode::BAD_REQUEST, "invalid size");
    };
    let Some(original) = store.path(id) else {
        return plain(StatusCode::NOT_FOUND, "unknown cover");
    };
    let cached = cache.join(size.to_string()).join(format!("{id}.jpg"));
    let body = match std::fs::read(&cached) {
        Ok(bytes) => bytes,
        Err(_) => match resize(&original, size) {
            Ok(bytes) => {
                let _ = write_atomic(&cached, &bytes);
                bytes
            }
            Err(_) => return plain(StatusCode::UNPROCESSABLE_ENTITY, "cover cannot be decoded"),
        },
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "max-age=31536000, immutable")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(body)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

fn resize(original: &Path, size: u32) -> anyhow::Result<Vec<u8>> {
    let image = image::open(original)?;
    let resized = image.resize(size, size, FilterType::Lanczos3).to_rgb8();
    let mut out = Cursor::new(Vec::new());
    resized.write_with_encoder(JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY))?;
    Ok(out.into_inner())
}

/// Temp file plus rename: parallel requests for one cover never expose half a file.
fn write_atomic(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = target.with_extension(format!(
        "{}.{:?}.tmp",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, target).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

fn plain(status: StatusCode, message: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(message.as_bytes().to_vec())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, RgbImage};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = RgbImage::from_fn(width, height, |x, y| image::Rgb([x as u8, y as u8, 90]));
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn setup() -> (tempfile::TempDir, CoverStore) {
        let root = tempfile::tempdir().unwrap();
        let store = CoverStore::new(root.path().join("original"));
        (root, store)
    }

    #[test]
    fn serves_resized_cover() {
        let (root, store) = setup();
        let id = store.store_bytes(&png(400, 400)).unwrap();
        let response = handle(&store, root.path(), &format!("/cover/{id}/96"));
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "max-age=31536000, immutable"
        );
        let decoded = image::load_from_memory(response.body()).unwrap();
        assert!(decoded.width() <= 96 && decoded.height() <= 96);
    }

    #[test]
    fn unknown_cover_is_404() {
        let (root, store) = setup();
        let response = handle(&store, root.path(), "/cover/deadbeef/96");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn bad_size_or_path_is_400() {
        let (root, store) = setup();
        let id = store.store_bytes(&png(400, 400)).unwrap();
        for path in [
            format!("/cover/{id}/100"),
            format!("/cover/{id}/abc"),
            format!("/cover/{id}"),
            format!("/other/{id}/96"),
            format!("/cover/{id}/96/extra"),
        ] {
            let response = handle(&store, root.path(), &path);
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        }
        // Traversal ids never reach the filesystem: the store rejects them as unknown.
        let response = handle(&store, root.path(), "/cover/../96");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn second_request_hits_disk_cache() {
        let (root, store) = setup();
        let id = store.store_bytes(&png(400, 400)).unwrap();
        let cached = root.path().join("300").join(format!("{id}.jpg"));
        assert!(!cached.exists());
        let first = handle(&store, root.path(), &format!("/cover/{id}/300"));
        assert!(cached.is_file());
        // A marker in the cache file proves the second call reads it instead of resizing.
        std::fs::write(&cached, b"cached").unwrap();
        let second = handle(&store, root.path(), &format!("/cover/{id}/300"));
        assert_eq!(second.body().as_slice(), b"cached");
        assert_ne!(first.body().as_slice(), b"cached");
    }
}
