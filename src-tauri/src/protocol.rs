//! The `prism://` scheme the frontend loads images from.
//!
//! `prism://localhost/cover/<coverId>/<size>` (on Windows the WebView sees
//! `http://prism.localhost/cover/<coverId>/<size>`) returns the cover resized to a square
//! bound of 96, 300 or 600 px as JPEG. Resized covers are cached on disk.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use tauri::http::{header, Request, Response, StatusCode};
use tauri::{Manager, Runtime, UriSchemeContext, UriSchemeResponder};

use crate::library::covers::CoverStore;
use crate::thumbs::{ResizeError, ResizeGate, SIZES};
use crate::AppState;

pub fn register<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    let path = request.uri().path().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        // A panic in a decoder must still answer the request.
        let response = catch_unwind(AssertUnwindSafe(|| {
            // A request that arrives before setup finished gets a retryable answer.
            let Some(state) = app.try_state::<AppState>() else {
                return no_store(StatusCode::SERVICE_UNAVAILABLE, "starting");
            };
            let store = &state.library.covers;
            match store.dir().parent() {
                Some(cache) => handle(&state.thumbs, store, cache, &path),
                None => plain(StatusCode::INTERNAL_SERVER_ERROR, "no cache dir"),
            }
        }))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "cover handler panicked"));
        responder.respond(response);
    });
}

/// Serves `/cover/<id>/<size>`. `path` is the URI path only; the query string is already
/// stripped by `Uri::path()`. `cache` is the directory holding one subfolder per size.
pub fn handle(
    gate: &ResizeGate,
    store: &CoverStore,
    cache: &Path,
    path: &str,
) -> Response<Vec<u8>> {
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
    let cached = match gate.get(store, cache, id, size) {
        Ok(path) => path,
        Err(ResizeError::Unknown) => return plain(StatusCode::NOT_FOUND, "unknown cover"),
        Err(ResizeError::Undecodable(_)) => {
            return plain(StatusCode::UNPROCESSABLE_ENTITY, "cover cannot be decoded")
        }
        Err(ResizeError::Internal(_)) => {
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "cover cannot be served")
        }
        // The server is offline: the web view must ask again later, not keep this answer.
        Err(ResizeError::Unreachable) => {
            return no_store(
                StatusCode::SERVICE_UNAVAILABLE,
                "cover server not reachable",
            )
        }
    };
    let Ok(body) = std::fs::read(&cached) else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "cover cannot be served");
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "max-age=31536000, immutable")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(body)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

fn plain(status: StatusCode, message: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(message.as_bytes().to_vec())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

/// A plain answer the web view must not cache.
fn no_store(status: StatusCode, message: &str) -> Response<Vec<u8>> {
    let mut response = plain(status, message);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, RgbImage};
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use crate::thumbs::RemoteCovers;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = RgbImage::from_fn(width, height, |x, y| image::Rgb([x as u8, y as u8, 90]));
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn gate() -> ResizeGate {
        ResizeGate::new(2)
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
        let response = handle(&gate(), &store, root.path(), &format!("/cover/{id}/96"));
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
        let response = handle(&gate(), &store, root.path(), "/cover/deadbeef/96");
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
            format!("/cover/{id}/96/"),
        ] {
            let response = handle(&gate(), &store, root.path(), &path);
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        }
        // Traversal ids never reach the filesystem: the store rejects them as unknown.
        let response = handle(&gate(), &store, root.path(), "/cover/../96");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn never_upscales() {
        let (root, store) = setup();
        let id = store.store_bytes(&png(50, 50)).unwrap();
        let response = handle(&gate(), &store, root.path(), &format!("/cover/{id}/300"));
        assert_eq!(response.status(), StatusCode::OK);
        let decoded = image::load_from_memory(response.body()).unwrap();
        assert!(decoded.width() <= 50 && decoded.height() <= 50);
    }

    #[test]
    fn transparent_cover_is_flattened_onto_surface() {
        let (root, store) = setup();
        let clear = image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 255, 255, 0]));
        let mut out = Cursor::new(Vec::new());
        clear.write_to(&mut out, ImageFormat::Png).unwrap();
        let id = store.store_bytes(&out.into_inner()).unwrap();
        let response = handle(&gate(), &store, root.path(), &format!("/cover/{id}/96"));
        let pixel = image::load_from_memory(response.body())
            .unwrap()
            .to_rgb8()
            .get_pixel(4, 4)
            .0;
        assert!(
            pixel
                .iter()
                .zip([0x14u8, 0x14, 0x18])
                .all(|(a, b)| a.abs_diff(b) <= 3),
            "{pixel:?}"
        );
    }

    #[test]
    fn undecodable_cover_is_422() {
        let (root, store) = setup();
        let mut bytes = png(10, 10);
        bytes.truncate(40);
        bytes.extend_from_slice(b"garbage garbage garbage");
        let id = store.store_bytes(&bytes).unwrap();
        let response = handle(&gate(), &store, root.path(), &format!("/cover/{id}/96"));
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// A server that is never reachable and counts how often it was asked.
    #[derive(Default)]
    struct DownServer(AtomicUsize);

    impl RemoteCovers for DownServer {
        fn fetch(&self, _: &str, _: u32) -> Result<Vec<u8>, ResizeError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(ResizeError::Unreachable)
        }
    }

    fn down_gate() -> (ResizeGate, Arc<DownServer>) {
        let server = Arc::new(DownServer::default());
        let gate = gate();
        gate.set_remote_for(
            crate::library::model::NAVIDROME_COVER_PREFIX,
            server.clone(),
        );
        (gate, server)
    }

    #[test]
    fn protocol_maps_unreachable_to_503_no_store() {
        let (root, store) = setup();
        let (gate, server) = down_gate();
        let response = handle(&gate, &store, root.path(), "/cover/nd-al-1_abc/300");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(server.0.load(Ordering::SeqCst), 1);
        assert!(!root.path().join("300").join("nd-al-1_abc.jpg").exists());
    }

    #[test]
    fn non_hex_local_ids_still_rejected() {
        let (root, store) = setup();
        let (gate, server) = down_gate();
        for path in [
            "/cover/xyz/96",
            "/cover/nd-/96",
            "/cover/ND-al-1/96",
            "/cover/nd-al%201/96",
            "/cover/nd-al:1/96",
            "/cover/nd-..%5Cx/96",
            "/cover/n-al-1/96",
        ] {
            let response = handle(&gate, &store, root.path(), path);
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
        assert_eq!(
            server.0.load(Ordering::SeqCst),
            0,
            "no id above reaches the server"
        );
    }

    #[test]
    fn second_request_hits_disk_cache() {
        let (root, store) = setup();
        let id = store.store_bytes(&png(400, 400)).unwrap();
        let cached = root.path().join("300").join(format!("{id}.jpg"));
        assert!(!cached.exists());
        let first = handle(&gate(), &store, root.path(), &format!("/cover/{id}/300"));
        assert!(cached.is_file());
        // A marker in the cache file proves the second call reads it instead of resizing.
        std::fs::write(&cached, b"cached").unwrap();
        let second = handle(&gate(), &store, root.path(), &format!("/cover/{id}/300"));
        assert_eq!(second.body().as_slice(), b"cached");
        assert_ne!(first.body().as_slice(), b"cached");
    }
}
