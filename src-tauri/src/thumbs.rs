//! Cover thumbnails: resizing, the on-disk cache, and the gate that keeps a burst of cover
//! requests from running dozens of decodes (or server fetches) at once.
//!
//! Local covers come from the cover store. Remote covers come through one [`RemoteCovers`]
//! per id prefix: Navidrome covers (`nd-<coverArt>`) from the server, station logos
//! (`rb-<hash>`) from the station's favicon or homepage. They are fetched at the size asked
//! for, then flattened, bounded and encoded like a local cover into the same cache.

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
use crate::library::model::{navidrome_cover_raw, NAVIDROME_COVER_PREFIX};
use crate::radio::model::{radio_cover_raw, RADIO_COVER_PREFIX};

/// The sizes covers are served at.
pub const SIZES: [u32; 3] = [96, 300, 600];
const JPEG_QUALITY: u8 = 85;

/// At most this many fetches of one remote run at once, unless it sets its own limit.
pub const REMOTE_FETCH_LIMIT: usize = 6;

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
    /// The server holding the cover cannot be asked now. Nothing is cached; a later request
    /// tries again.
    Unreachable,
}

/// Where the covers of one id prefix come from.
pub trait RemoteCovers: Send + Sync {
    /// The image bytes of the cover `raw` (the id without its prefix), asked for at `size` px.
    fn fetch(&self, raw: &str, size: u32) -> Result<Vec<u8>, ResizeError>;

    /// How many fetches of this remote may run at once.
    fn fetch_limit(&self) -> usize {
        REMOTE_FETCH_LIMIT
    }
}

/// A remote and the permits its fetches share.
struct RemoteSlot {
    remote: Arc<dyn RemoteCovers>,
    fetches: Arc<Semaphore>,
}

/// One resize in flight; later requests for the same cover and size wait on it.
struct InFlight {
    result: Mutex<Option<Result<PathBuf, ResizeError>>>,
    done: Condvar,
}

impl InFlight {
    fn new() -> InFlight {
        InFlight {
            result: Mutex::new(None),
            done: Condvar::new(),
        }
    }
}

/// A counting semaphore.
struct Semaphore {
    permits: Mutex<usize>,
    available: Condvar,
}

/// Returns a semaphore permit when dropped.
struct Permit<'a>(&'a Semaphore);

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        *lock(&self.0.permits) += 1;
        self.0.available.notify_one();
    }
}

impl Semaphore {
    fn new(limit: usize) -> Semaphore {
        Semaphore {
            permits: Mutex::new(limit.max(1)),
            available: Condvar::new(),
        }
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

/// Limits how many resizes run at once and lets concurrent requests for the same
/// `(id, size)` share one resize (or one server fetch).
pub struct ResizeGate {
    inflight: Mutex<HashMap<(String, u32), Arc<InFlight>>>,
    resizes: Semaphore,
    remotes: Mutex<HashMap<&'static str, RemoteSlot>>,
    /// Bumped per prefix by [`ResizeGate::purge_remote`]. A remote cover is written only while
    /// the generation of its prefix it was fetched in is still current; the check and the
    /// write happen under this lock, so nothing of an old server lands after a purge.
    remote_generation: Mutex<HashMap<&'static str, u64>>,
}

/// Locks, ignoring poisoning: the guarded data stays consistent across a panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Ends a flight: leaves the map and wakes every waiter, also when the leader unwinds (then
/// with `Internal`, and the next request tries again).
struct Flight<'a> {
    gate: &'a ResizeGate,
    key: (String, u32),
    flight: Arc<InFlight>,
    outcome: Option<Result<PathBuf, ResizeError>>,
}

impl Drop for Flight<'_> {
    fn drop(&mut self) {
        let outcome = self
            .outcome
            .take()
            .unwrap_or_else(|| Err(ResizeError::Internal("cover request panicked".into())));
        // Publish the result and leave the map together, so a request arriving now either
        // joins this flight or finds the file on disk.
        let mut map = lock(&self.gate.inflight);
        map.remove(&self.key);
        *lock(&self.flight.result) = Some(outcome);
        self.flight.done.notify_all();
        drop(map);
    }
}

/// Where the original of a cover comes from.
enum Origin<'a> {
    Local(PathBuf),
    /// The prefix and the id without it.
    Remote(&'static str, &'a str),
}

impl ResizeGate {
    pub fn new(limit: usize) -> Self {
        Self {
            inflight: Mutex::new(HashMap::new()),
            resizes: Semaphore::new(limit),
            remotes: Mutex::new(HashMap::new()),
            remote_generation: Mutex::new(HashMap::new()),
        }
    }

    /// The default limit: at most 4 resizes, fewer on small machines.
    pub fn default_limit() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(4)
    }

    /// Where the covers of `prefix` (`nd-` or `rb-`) are fetched from. Until this is set they
    /// are `Unreachable`.
    pub fn set_remote_for(&self, prefix: &'static str, remote: Arc<dyn RemoteCovers>) {
        let fetches = Arc::new(Semaphore::new(remote.fetch_limit()));
        lock(&self.remotes).insert(prefix, RemoteSlot { remote, fetches });
    }

    /// Forgets every cached cover of `prefix`: deletes its files under every size folder of
    /// `cache`, and a fetch of that prefix still running writes nothing. Called after a server
    /// switch or a disconnect, so covers of the old server never show for the new one; the
    /// covers of other prefixes stay.
    pub fn purge_remote(&self, cache: &Path, prefix: &'static str) {
        *lock(&self.remote_generation).entry(prefix).or_insert(0) += 1;
        for size in SIZES {
            let Ok(entries) = std::fs::read_dir(cache.join(size.to_string())) else {
                continue;
            };
            for entry in entries.flatten() {
                if !entry.file_name().to_string_lossy().starts_with(prefix) {
                    continue;
                }
                if let Err(err) = std::fs::remove_file(entry.path()) {
                    eprintln!("removing a cached server cover failed: {err}");
                }
            }
        }
    }

    /// The cached JPEG of cover `id` at `size` px, resized (or fetched from the server) now
    /// when it is not cached yet.
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
        // Server cover ids are checked against `[A-Za-z0-9_.-]`, logo ids are 20 hex digits;
        // everything else must be a hex id of the store.
        let origin = if let Some(raw) = navidrome_cover_raw(id) {
            Origin::Remote(NAVIDROME_COVER_PREFIX, raw)
        } else if id.starts_with(RADIO_COVER_PREFIX) {
            Origin::Remote(
                RADIO_COVER_PREFIX,
                radio_cover_raw(id).ok_or(ResizeError::Unknown)?,
            )
        } else {
            Origin::Local(store.path(id).ok_or(ResizeError::Unknown)?)
        };
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
                    let flight = Arc::new(InFlight::new());
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

        let mut guard = Flight {
            gate: self,
            key,
            flight,
            outcome: None,
        };
        let outcome = match origin {
            Origin::Local(original) => self.run(&original, &cached, size, &resize),
            Origin::Remote(prefix, raw) => self.run_remote(prefix, raw, &cached, size),
        };
        guard.outcome = Some(outcome.clone());
        drop(guard);
        outcome
    }

    fn run(
        &self,
        original: &Path,
        cached: &Path,
        size: u32,
        resize: &impl Fn(&Path, u32) -> Result<Vec<u8>, ResizeError>,
    ) -> Result<PathBuf, ResizeError> {
        let _permit = self.resizes.acquire();
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

    /// Fetches a remote cover (holding a fetch permit of its remote, not a resize permit, while
    /// it waits on the network), then encodes it like a local cover. Failures write nothing.
    fn run_remote(
        &self,
        prefix: &'static str,
        raw: &str,
        cached: &Path,
        size: u32,
    ) -> Result<PathBuf, ResizeError> {
        let Some((remote, fetches)) = lock(&self.remotes)
            .get(prefix)
            .map(|slot| (Arc::clone(&slot.remote), Arc::clone(&slot.fetches)))
        else {
            return Err(ResizeError::Unreachable);
        };
        let generation_of = |map: &HashMap<&'static str, u64>| map.get(prefix).copied();
        let generation = generation_of(&lock(&self.remote_generation));
        let bytes = {
            let _fetch = fetches.acquire();
            if cached.is_file() {
                return Ok(cached.to_path_buf());
            }
            catch_unwind(AssertUnwindSafe(|| remote.fetch(raw, size)))
                .unwrap_or_else(|_| Err(ResizeError::Internal("cover fetch panicked".into())))?
        };
        let jpeg = {
            let _permit = self.resizes.acquire();
            catch_unwind(AssertUnwindSafe(|| encode(&bytes, size)))
                .unwrap_or_else(|_| Err(ResizeError::Internal("resize panicked".into())))?
        };
        let current = lock(&self.remote_generation);
        if generation_of(&current) != generation {
            // The server was switched while this fetch ran.
            return Err(ResizeError::Unreachable);
        }
        write_atomic(cached, &jpeg).map_err(|e| ResizeError::Internal(e.to_string()))?;
        drop(current);
        Ok(cached.to_path_buf())
    }
}

/// Neutral cover for tracks without one: the app icon on the surface color.
/// Cached at `<cache>/600/_placeholder-<8 hex>.jpg`, where the hex is a short hash of the
/// icon bytes so a new icon is not hidden by an old cache file; the leading underscore
/// cannot collide with a cover id (those are hex, or start with `nd-` or `rb-`).
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
    encode(&bytes, size)
}

/// Decodes an image, bounds it to `size` px, flattens it onto the surface color and encodes
/// it as JPEG.
fn encode(bytes: &[u8], size: u32) -> Result<Vec<u8>, ResizeError> {
    let image = image::load_from_memory(bytes).map_err(|e| match e {
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

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

    /// A server stand-in: answers with `answer`, counts calls per size, can hold calls open.
    struct FakeRemote {
        calls: Mutex<Vec<(String, u32)>>,
        delay: Duration,
        answer: Mutex<Result<Vec<u8>, ResizeError>>,
        running: AtomicUsize,
        peak: AtomicUsize,
        hold: AtomicBool,
        panic_once: AtomicBool,
    }

    impl FakeRemote {
        fn new(delay: Duration) -> Arc<FakeRemote> {
            Arc::new(FakeRemote {
                calls: Mutex::new(Vec::new()),
                delay,
                answer: Mutex::new(Ok(png(7))),
                running: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                hold: AtomicBool::new(false),
                panic_once: AtomicBool::new(false),
            })
        }

        fn calls(&self) -> Vec<(String, u32)> {
            lock(&self.calls).clone()
        }

        fn answer(&self, answer: Result<Vec<u8>, ResizeError>) {
            *lock(&self.answer) = answer;
        }
    }

    impl RemoteCovers for FakeRemote {
        fn fetch(&self, cover_art: &str, size: u32) -> Result<Vec<u8>, ResizeError> {
            lock(&self.calls).push((cover_art.to_owned(), size));
            let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.hold.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            self.running.fetch_sub(1, Ordering::SeqCst);
            if self.panic_once.swap(false, Ordering::SeqCst) {
                panic!("fetch panicked");
            }
            lock(&self.answer).clone()
        }
    }

    fn remote_gate(limit: usize, remote: &Arc<FakeRemote>) -> ResizeGate {
        let gate = ResizeGate::new(limit);
        gate.set_remote_for(
            NAVIDROME_COVER_PREFIX,
            Arc::clone(remote) as Arc<dyn RemoteCovers>,
        );
        gate
    }

    const FAVICON: &str = "rb-0123456789abcdef0123";

    #[test]
    fn navidrome_purge_keeps_favicons() {
        let (root, store) = setup();
        let server = FakeRemote::new(Duration::ZERO);
        let favicons = FakeRemote::new(Duration::ZERO);
        let gate = remote_gate(2, &server);
        gate.set_remote_for(
            RADIO_COVER_PREFIX,
            Arc::clone(&favicons) as Arc<dyn RemoteCovers>,
        );
        let server_file = gate.get(&store, root.path(), "nd-al-1", 300).unwrap();
        let favicon_file = gate.get(&store, root.path(), FAVICON, 300).unwrap();
        assert_eq!(
            favicon_file,
            root.path().join("300").join(format!("{FAVICON}.jpg"))
        );
        // Each remote got its own request, by the id without the prefix.
        assert_eq!(server.calls(), vec![("al-1".to_string(), 300)]);
        assert_eq!(
            favicons.calls(),
            vec![("0123456789abcdef0123".to_string(), 300)]
        );

        gate.purge_remote(root.path(), NAVIDROME_COVER_PREFIX);
        assert!(!server_file.exists());
        assert!(favicon_file.is_file(), "favicons stay");
        gate.purge_remote(root.path(), RADIO_COVER_PREFIX);
        assert!(!favicon_file.exists());
    }

    #[test]
    fn a_purge_of_one_prefix_lets_the_other_write() {
        let (root, store) = setup();
        let server = FakeRemote::new(Duration::ZERO);
        let favicons = FakeRemote::new(Duration::ZERO);
        favicons.hold.store(true, Ordering::SeqCst);
        let gate = remote_gate(2, &server);
        gate.set_remote_for(
            RADIO_COVER_PREFIX,
            Arc::clone(&favicons) as Arc<dyn RemoteCovers>,
        );
        let result = std::thread::scope(|scope| {
            let request = scope.spawn(|| gate.get(&store, root.path(), FAVICON, 96));
            let deadline = Instant::now() + Duration::from_secs(5);
            while favicons.calls().is_empty() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            gate.purge_remote(root.path(), NAVIDROME_COVER_PREFIX);
            favicons.hold.store(false, Ordering::SeqCst);
            request.join().unwrap()
        });
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn bad_favicon_ids_never_reach_a_remote() {
        let (root, store) = setup();
        let favicons = FakeRemote::new(Duration::ZERO);
        let gate = ResizeGate::new(2);
        gate.set_remote_for(
            RADIO_COVER_PREFIX,
            Arc::clone(&favicons) as Arc<dyn RemoteCovers>,
        );
        for id in ["rb-../x", "rb-0123", "rb-0123456789ABCDEF0123", "rb-"] {
            assert_eq!(
                gate.get(&store, root.path(), id, 96),
                Err(ResizeError::Unknown),
                "{id}"
            );
        }
        assert!(favicons.calls().is_empty());
        // Without a favicon remote, a good id is unreachable, not unknown.
        let bare = ResizeGate::new(2);
        assert_eq!(
            bare.get(&store, root.path(), FAVICON, 96),
            Err(ResizeError::Unreachable)
        );
    }

    #[test]
    fn a_remote_sets_its_own_fetch_limit() {
        struct Limited(Arc<FakeRemote>);
        impl RemoteCovers for Limited {
            fn fetch(&self, raw: &str, size: u32) -> Result<Vec<u8>, ResizeError> {
                self.0.fetch(raw, size)
            }
            fn fetch_limit(&self) -> usize {
                2
            }
        }
        let (root, store) = setup();
        let remote = FakeRemote::new(Duration::from_millis(40));
        let gate = ResizeGate::new(4);
        gate.set_remote_for(RADIO_COVER_PREFIX, Arc::new(Limited(Arc::clone(&remote))));
        std::thread::scope(|scope| {
            for n in 0..8 {
                let gate = &gate;
                let store = &store;
                let root = root.path();
                scope.spawn(move || {
                    let id = format!("rb-{n:020x}");
                    gate.get(store, root, &id, 96).unwrap();
                });
            }
        });
        let peak = remote.peak.load(Ordering::SeqCst);
        assert!(peak <= 2, "peak {peak}");
        assert_eq!(remote.calls().len(), 8);
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
    fn remote_cover_fetched_once_per_size() {
        let (root, store) = setup();
        let remote = FakeRemote::new(Duration::from_millis(100));
        let gate = remote_gate(4, &remote);
        let paths: Vec<PathBuf> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..16)
                .map(|_| scope.spawn(|| gate.get(&store, root.path(), "nd-al-1", 300).unwrap()))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(remote.calls(), vec![("al-1".to_string(), 300)]);
        assert!(paths.iter().all(|p| *p == paths[0]));
        assert_eq!(paths[0], root.path().join("300").join("nd-al-1.jpg"));
        // Encoded like a local cover: a JPEG.
        let written = std::fs::read(&paths[0]).unwrap();
        assert_eq!(&written[..2], &[0xFF, 0xD8]);

        // Cached: no second fetch at 300; 96 is a fetch of its own.
        gate.get(&store, root.path(), "nd-al-1", 300).unwrap();
        gate.get(&store, root.path(), "nd-al-1", 96).unwrap();
        assert_eq!(
            remote.calls(),
            vec![("al-1".to_string(), 300), ("al-1".to_string(), 96)]
        );
    }

    #[test]
    fn remote_fetches_are_capped_and_hold_no_resize_permit() {
        let (root, store) = setup();
        let remote = FakeRemote::new(Duration::from_millis(60));
        // One resize permit: a fetch holding it would cap the fetches at one.
        let gate = remote_gate(1, &remote);
        std::thread::scope(|scope| {
            for n in 0..18 {
                let gate = &gate;
                let store = &store;
                let root = root.path();
                scope.spawn(move || {
                    gate.get(store, root, &format!("nd-al-{n}"), 96).unwrap();
                });
            }
        });
        let peak = remote.peak.load(Ordering::SeqCst);
        assert!(peak <= REMOTE_FETCH_LIMIT, "peak {peak}");
        assert!(peak > 1, "fetches ran one by one");
        assert_eq!(remote.calls().len(), 18);
    }

    #[test]
    fn offline_remote_is_not_cached() {
        let (root, store) = setup();
        let remote = FakeRemote::new(Duration::ZERO);
        let gate = remote_gate(2, &remote);
        remote.answer(Err(ResizeError::Unreachable));
        assert_eq!(
            gate.get(&store, root.path(), "nd-al-1", 300),
            Err(ResizeError::Unreachable)
        );
        let cached = root.path().join("300").join("nd-al-1.jpg");
        assert!(!cached.exists());
        // Unknown writes nothing either.
        remote.answer(Err(ResizeError::Unknown));
        assert_eq!(
            gate.get(&store, root.path(), "nd-al-1", 300),
            Err(ResizeError::Unknown)
        );
        assert!(!cached.exists());

        remote.answer(Ok(png(9)));
        assert_eq!(
            gate.get(&store, root.path(), "nd-al-1", 300),
            Ok(cached.clone())
        );
        assert!(cached.is_file());
        assert_eq!(remote.calls().len(), 3);
    }

    #[test]
    fn without_a_remote_server_covers_are_unreachable() {
        let (root, store) = setup();
        let gate = ResizeGate::new(2);
        assert_eq!(
            gate.get(&store, root.path(), "nd-al-1", 300),
            Err(ResizeError::Unreachable)
        );
        assert!(!root.path().join("300").exists());
    }

    #[test]
    fn panic_in_fetch_does_not_leak_inflight() {
        let (root, store) = setup();
        let remote = FakeRemote::new(Duration::from_millis(200));
        remote.panic_once.store(true, Ordering::SeqCst);
        let gate = remote_gate(2, &remote);
        let results: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..6)
                .map(|_| scope.spawn(|| gate.get(&store, root.path(), "nd-al-1", 300)))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert!(
            results
                .iter()
                .all(|r| matches!(r, Err(ResizeError::Internal(_)))),
            "{results:?}"
        );
        assert!(lock(&gate.inflight).is_empty());
        assert_eq!(remote.calls().len(), 1);
        // The next call fetches again, and both permits came back.
        assert!(gate.get(&store, root.path(), "nd-al-1", 300).is_ok());
        assert_eq!(remote.calls().len(), 2);
    }

    #[test]
    fn unwinding_leader_wakes_waiters_and_leaves_the_map() {
        let gate = ResizeGate::new(1);
        let key = ("nd-al-1".to_string(), 96);
        let flight = Arc::new(InFlight::new());
        lock(&gate.inflight).insert(key.clone(), Arc::clone(&flight));
        let unwound = catch_unwind(AssertUnwindSafe(|| {
            let _guard = Flight {
                gate: &gate,
                key,
                flight: Arc::clone(&flight),
                outcome: None,
            };
            panic!("leader panicked");
        }));
        assert!(unwound.is_err());
        assert!(lock(&gate.inflight).is_empty());
        assert!(matches!(
            *lock(&flight.result),
            Some(Err(ResizeError::Internal(_)))
        ));
    }

    #[test]
    fn purge_removes_server_covers_only() {
        let (root, store) = setup();
        let local = store.store_bytes(&png(4)).unwrap();
        let remote = FakeRemote::new(Duration::ZERO);
        let gate = remote_gate(2, &remote);
        let mut server_files = Vec::new();
        for size in SIZES {
            server_files.push(gate.get(&store, root.path(), "nd-al-1", size).unwrap());
        }
        let local_file = gate.get(&store, root.path(), &local, 300).unwrap();
        let placeholder = placeholder_cover(root.path()).unwrap();

        gate.purge_remote(root.path(), NAVIDROME_COVER_PREFIX);
        assert!(server_files.iter().all(|file| !file.exists()));
        assert!(local_file.is_file());
        assert!(placeholder.is_file());
        // A purge of an empty cache is fine.
        gate.purge_remote(&root.path().join("missing"), NAVIDROME_COVER_PREFIX);
    }

    #[test]
    fn purge_during_a_fetch_writes_nothing() {
        let (root, store) = setup();
        let remote = FakeRemote::new(Duration::ZERO);
        remote.hold.store(true, Ordering::SeqCst);
        let gate = remote_gate(2, &remote);
        let result = std::thread::scope(|scope| {
            let request = scope.spawn(|| gate.get(&store, root.path(), "nd-al-1", 300));
            let deadline = Instant::now() + Duration::from_secs(5);
            while remote.calls().is_empty() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            gate.purge_remote(root.path(), NAVIDROME_COVER_PREFIX);
            remote.hold.store(false, Ordering::SeqCst);
            request.join().unwrap()
        });
        assert_eq!(result, Err(ResizeError::Unreachable));
        assert!(!root.path().join("300").join("nd-al-1.jpg").exists());
        // The next request fetches for the new server and caches.
        assert!(gate.get(&store, root.path(), "nd-al-1", 300).is_ok());
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
