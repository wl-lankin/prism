//! Cover thumbnails: resizing, the on-disk cache, and the gate that keeps a burst of cover
//! requests from running dozens of decodes at once.

use std::collections::HashMap;
use std::io::Cursor;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageError, Rgb, RgbImage};
use sha1::{Digest, Sha1};

use crate::library::covers::CoverStore;

/// The sizes covers are served at.
pub const SIZES: [u32; 3] = [96, 300, 600];
const JPEG_QUALITY: u8 = 85;

/// Background for transparent covers: the `--surface` token.
const SURFACE: Rgb<u8> = Rgb([0x14, 0x14, 0x18]);

const ICON: &[u8] = include_bytes!("../icons/icon.png");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResizeError {
    /// No such cover, or a size that is not served.
    Unknown,
    /// The bytes are not a decodable image: the client cannot fix this, but it is not our fault.
    Undecodable(String),
    /// Disk or encoder failure.
    Internal(String),
}

/// One resize in flight; later requests for the same cover and size wait on it.
struct InFlight {
    result: Mutex<Option<Result<PathBuf, ResizeError>>>,
    done: Condvar,
}

/// Limits how many resizes run at once and lets concurrent requests for the same
/// `(id, size)` share one resize.
pub struct ResizeGate {
    inflight: Mutex<HashMap<(String, u32), Arc<InFlight>>>,
    permits: Mutex<usize>,
    available: Condvar,
}

/// Locks, ignoring poisoning: the guarded data stays consistent across a panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Returns a semaphore permit when dropped.
struct Permit<'a>(&'a ResizeGate);

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        *lock(&self.0.permits) += 1;
        self.0.available.notify_one();
    }
}

impl ResizeGate {
    pub fn new(limit: usize) -> Self {
        Self {
            inflight: Mutex::new(HashMap::new()),
            permits: Mutex::new(limit.max(1)),
            available: Condvar::new(),
        }
    }

    /// The default limit: at most 4 resizes, fewer on small machines.
    pub fn default_limit() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(4)
    }

    /// The cached JPEG of cover `id` at `size` px, resized now when it is not cached yet.
    pub fn get(
        &self,
        store: &CoverStore,
        cache: &Path,
        id: &str,
        size: u32,
    ) -> Result<PathBuf, ResizeError> {
        self.get_with(store, cache, id, size, resize)
    }

    pub(crate) fn get_with(
        &self,
        store: &CoverStore,
        cache: &Path,
        id: &str,
        size: u32,
        resize: impl Fn(&Path, u32) -> Result<Vec<u8>, ResizeError>,
    ) -> Result<PathBuf, ResizeError> {
        if !SIZES.contains(&size) {
            return Err(ResizeError::Unknown);
        }
        let original = store.path(id).ok_or(ResizeError::Unknown)?;
        let cached = cache.join(size.to_string()).join(format!("{id}.jpg"));
        if cached.is_file() {
            return Ok(cached);
        }

        let key = (id.to_string(), size);
        let (flight, leader) = {
            let mut map = lock(&self.inflight);
            match map.get(&key) {
                Some(existing) => (Arc::clone(existing), false),
                None => {
                    let flight = Arc::new(InFlight {
                        result: Mutex::new(None),
                        done: Condvar::new(),
                    });
                    map.insert(key.clone(), Arc::clone(&flight));
                    (flight, true)
                }
            }
        };

        if !leader {
            let mut slot = lock(&flight.result);
            while slot.is_none() {
                slot = flight
                    .done
                    .wait(slot)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            return slot.clone().unwrap_or(Err(ResizeError::Unknown));
        }

        let outcome = self.run(&original, &cached, size, &resize);
        // Publish the result and leave the map together, so a request arriving now either
        // joins this flight or finds the file on disk.
        let mut map = lock(&self.inflight);
        map.remove(&key);
        *lock(&flight.result) = Some(outcome.clone());
        flight.done.notify_all();
        drop(map);
        outcome
    }

    fn run(
        &self,
        original: &Path,
        cached: &Path,
        size: u32,
        resize: &impl Fn(&Path, u32) -> Result<Vec<u8>, ResizeError>,
    ) -> Result<PathBuf, ResizeError> {
        let _permit = self.acquire();
        // Another flight for this cover may have finished between the first check and now.
        if cached.is_file() {
            return Ok(cached.to_path_buf());
        }
        // A panic in a decoder must still release the waiters.
        let bytes = catch_unwind(AssertUnwindSafe(|| resize(original, size)))
            .unwrap_or_else(|_| Err(ResizeError::Internal("resize panicked".into())))?;
        write_atomic(cached, &bytes).map_err(|e| ResizeError::Internal(e.to_string()))?;
        Ok(cached.to_path_buf())
    }

    fn acquire(&self) -> Permit<'_> {
        let mut permits = lock(&self.permits);
        while *permits == 0 {
            permits = self
                .available
                .wait(permits)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        *permits -= 1;
        Permit(self)
    }
}

/// Neutral cover for tracks without one: the app icon on the surface color.
/// Cached at `<cache>/600/_placeholder-<8 hex>.jpg`, where the hex is a short hash of the
/// icon bytes so a new icon is not hidden by an old cache file; the leading underscore
/// cannot collide with a cover id (those are hex).
pub fn placeholder_cover(cache: &Path) -> Option<PathBuf> {
    let hash: String = Sha1::digest(ICON)
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let target = cache.join("600").join(format!("_placeholder-{hash}.jpg"));
    if target.is_file() {
        return Some(target);
    }
    let icon = image::load_from_memory(ICON).ok()?;
    let side = 600u32;
    let logo = flatten(&icon.resize(side / 2, side / 2, FilterType::Lanczos3));
    let mut canvas = RgbImage::from_pixel(side, side, SURFACE);
    let offset = ((side - logo.width()) / 2, (side - logo.height()) / 2);
    image::imageops::replace(&mut canvas, &logo, offset.0 as i64, offset.1 as i64);
    let mut out = Cursor::new(Vec::new());
    canvas
        .write_with_encoder(JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY))
        .ok()?;
    write_atomic(&target, &out.into_inner()).ok()?;
    Some(target)
}

fn flatten(image: &DynamicImage) -> RgbImage {
    let rgba = image.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let mix = |fg: u8, bg: u8| {
            ((fg as u32 * a as u32 + bg as u32 * (255 - a as u32) + 127) / 255) as u8
        };
        Rgb([mix(r, SURFACE[0]), mix(g, SURFACE[1]), mix(b, SURFACE[2])])
    })
}

fn resize(original: &Path, size: u32) -> Result<Vec<u8>, ResizeError> {
    let bytes = std::fs::read(original).map_err(|e| ResizeError::Internal(e.to_string()))?;
    let image = image::load_from_memory(&bytes).map_err(|e| match e {
        ImageError::IoError(io) => ResizeError::Internal(io.to_string()),
        other => ResizeError::Undecodable(other.to_string()),
    })?;
    // Never upscale: a small cover keeps its own size.
    let image = if image.width() <= size && image.height() <= size {
        image
    } else {
        image.resize(size, size, FilterType::Lanczos3)
    };
    let flat = flatten(&image);
    let mut out = Cursor::new(Vec::new());
    flat.write_with_encoder(JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY))
        .map_err(|e| ResizeError::Internal(e.to_string()))?;
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
    std::fs::write(&temp, bytes)
        .and_then(|()| std::fs::rename(&temp, target))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&temp);
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn png(seed: u8) -> Vec<u8> {
        let image = RgbImage::from_fn(8, 8, |x, y| Rgb([x as u8, y as u8, seed]));
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn setup() -> (tempfile::TempDir, CoverStore) {
        let root = tempfile::tempdir().unwrap();
        let store = CoverStore::new(root.path().join("original"));
        (root, store)
    }

    #[test]
    fn same_cover_is_resized_once() {
        let (root, store) = setup();
        let id = store.store_bytes(&png(1)).unwrap();
        let gate = ResizeGate::new(4);
        let calls = AtomicUsize::new(0);
        let paths: Vec<PathBuf> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..16)
                .map(|_| {
                    scope.spawn(|| {
                        gate.get_with(&store, root.path(), &id, 300, |_, _| {
                            calls.fetch_add(1, Ordering::SeqCst);
                            std::thread::sleep(Duration::from_millis(100));
                            Ok(vec![1, 2, 3])
                        })
                        .unwrap()
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(paths.iter().all(|p| *p == paths[0]));
        assert!(paths[0].is_file());
    }

    #[test]
    fn limit_caps_parallel_resizes() {
        let (root, store) = setup();
        let ids: Vec<String> = (0..32u8)
            .map(|seed| store.store_bytes(&png(seed)).unwrap())
            .collect();
        let gate = ResizeGate::new(3);
        let running = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for id in &ids {
                scope.spawn(|| {
                    gate.get_with(&store, root.path(), id, 96, |_, _| {
                        let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(15));
                        running.fetch_sub(1, Ordering::SeqCst);
                        Ok(vec![9])
                    })
                    .unwrap();
                });
            }
        });
        let peak = peak.load(Ordering::SeqCst);
        assert!(peak <= 3, "peak {peak}");
    }

    #[test]
    fn error_is_shared_then_retried() {
        let (root, store) = setup();
        let id = store.store_bytes(&png(2)).unwrap();
        let gate = ResizeGate::new(4);
        let calls = AtomicUsize::new(0);
        let arrived = AtomicUsize::new(0);
        let failing = |_: &Path, _: u32| {
            calls.fetch_add(1, Ordering::SeqCst);
            // Hold the flight open until every caller has started, then let them reach the wait.
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while arrived.load(Ordering::SeqCst) < 8 && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::thread::sleep(Duration::from_millis(400));
            Err(ResizeError::Undecodable("bad".into()))
        };
        let results: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        arrived.fetch_add(1, Ordering::SeqCst);
                        gate.get_with(&store, root.path(), &id, 300, failing)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(results
            .iter()
            .all(|r| matches!(r, Err(ResizeError::Undecodable(_)))));

        let retried = gate.get_with(&store, root.path(), &id, 300, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![1])
        });
        assert!(retried.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn panicking_resize_answers_every_waiter() {
        let (root, store) = setup();
        let id = store.store_bytes(&png(3)).unwrap();
        let gate = ResizeGate::new(1);
        let result = gate.get_with(&store, root.path(), &id, 96, |_, _| panic!("boom"));
        assert!(matches!(result, Err(ResizeError::Internal(_))));
        // The permit came back.
        assert!(gate
            .get_with(&store, root.path(), &id, 96, |_, _| Ok(vec![1]))
            .is_ok());
    }

    #[test]
    fn placeholder_name_carries_icon_hash() {
        let (root, _store) = setup();
        let path = placeholder_cover(root.path()).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let hex = name
            .strip_prefix("_placeholder-")
            .and_then(|rest| rest.strip_suffix(".jpg"))
            .unwrap();
        assert_eq!(hex.len(), 8);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
        let decoded = image::load_from_memory(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (600, 600));
        assert_eq!(placeholder_cover(root.path()), Some(path));
    }
}
