//! "Open with Prism" and the single instance: files named on the command line (Windows) or
//! in `RunEvent::Opened` (macOS) play in the running instance. The registry and plist
//! entries that put Prism into Explorer's and Finder's "Open with" menus live in
//! `windows/hooks.nsh` and `Info.plist`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use super::mini::{self, Anchor};
use crate::library::scan::is_audio_file;
use crate::player::{DroppedFiles, PlayerHandle, ReadOptions};
use crate::AppState;

/// The audio files among a command line. Skips `args[0]` (the executable) and `--flags`,
/// resolves relative paths against `cwd` and keeps what has an audio extension. It never
/// touches the file system, so a slow share cannot hang it.
pub fn paths_from_args(args: &[String], cwd: &Path) -> Vec<String> {
    args.iter()
        .skip(1)
        .filter(|arg| !arg.starts_with("--"))
        .map(|arg| {
            let path = Path::new(arg);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            }
        })
        .filter(|path| is_audio_file(path))
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

/// The audio files among `file://` URLs (macOS hands "Open with" files over this way).
/// Other schemes are ignored.
pub fn paths_from_urls(urls: &[tauri::Url]) -> Vec<String> {
    urls.iter()
        .filter(|url| url.scheme() == "file")
        .filter_map(|url| {
            let path = percent_decode(url.path())?;
            let host = url
                .host_str()
                .filter(|host| !host.is_empty() && !host.eq_ignore_ascii_case("localhost"));
            let full = match host {
                // `file://server/share/a.flac` is a UNC path.
                Some(host) => format!("//{host}{path}"),
                None => strip_drive_slash(path),
            };
            Some(PathBuf::from(full))
        })
        .filter(|path| is_audio_file(path))
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

/// `/C:/Music/a.flac` is `C:/Music/a.flac`; a Unix path stays as it is.
fn strip_drive_slash(path: String) -> String {
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        path[1..].to_string()
    } else {
        path
    }
}

/// Decodes `%XX` escapes; `None` when the result is not UTF-8.
fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// Reads `paths` (network paths allowed: the user picked them in Explorer) and plays them.
/// Returns how many tracks were queued; with none, the player is untouched.
fn play_files(
    dropped: &DroppedFiles,
    player: &PlayerHandle,
    paths: &[String],
) -> Result<u32, String> {
    let tracks = dropped.read_with(
        paths,
        ReadOptions {
            allow_network: true,
        },
    );
    dropped.commit_and_play(tracks, |ids| player.play(ids, 0))
}

/// Whether the user has no main window in front of them: it is closed, hidden or minimized.
/// One place to change when the shell state tracks "closed" itself.
fn main_is_away(app: &AppHandle) -> bool {
    match app.get_webview_window("main") {
        Some(window) => {
            !window.is_visible().unwrap_or(false) || window.is_minimized().unwrap_or(false)
        }
        None => true,
    }
}

/// How long after the latest arrival the collected files play. Selecting several files in
/// Explorer starts one process per file, each forwarding its own argv.
const QUIET: Duration = Duration::from_millis(300);
/// The longest the first arrival waits, however steadily more files keep coming.
const CAP: Duration = Duration::from_secs(1);
/// How often the worker looks at the queue.
const TICK: Duration = Duration::from_millis(50);

/// Files that arrived close together, in arrival order. Pure (the caller passes the
/// clock), so the timing is testable.
#[derive(Default)]
struct Debounce {
    paths: Vec<String>,
    first: Option<Instant>,
    last: Option<Instant>,
    /// A worker thread is draining this queue.
    running: bool,
}

impl Debounce {
    /// Adds `paths`. Returns true when the caller must start a worker.
    fn push(&mut self, now: Instant, paths: Vec<String>) -> bool {
        if paths.is_empty() {
            return false;
        }
        self.first.get_or_insert(now);
        self.last = Some(now);
        self.paths.extend(paths);
        !std::mem::replace(&mut self.running, true)
    }

    fn is_due(&self, now: Instant) -> bool {
        match (self.first, self.last) {
            (Some(first), Some(last)) => {
                now.duration_since(last) >= QUIET || now.duration_since(first) >= CAP
            }
            _ => false,
        }
    }

    /// Everything collected so far, once the burst is over.
    fn take_due(&mut self, now: Instant) -> Option<Vec<String>> {
        if !self.is_due(now) {
            return None;
        }
        self.first = None;
        self.last = None;
        Some(std::mem::take(&mut self.paths))
    }
}

static PENDING: Mutex<Debounce> = Mutex::new(Debounce {
    paths: Vec::new(),
    first: None,
    last: None,
    running: false,
});

fn pending() -> MutexGuard<'static, Debounce> {
    PENDING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Queues `paths` to play in this instance. Files arriving within a short window play
/// together as one queue; files that arrive before the app state exists wait for it. When
/// the main window is closed or hidden, the mini player appears (without focus) so the
/// user sees that something plays.
pub fn play_paths(app: &AppHandle, paths: Vec<String>) {
    if !pending().push(Instant::now(), paths) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(TICK);
        let batch = {
            let mut queue = pending();
            if queue.paths.is_empty() {
                queue.running = false;
                return;
            }
            if app.try_state::<AppState>().is_none() {
                continue;
            }
            queue.take_due(Instant::now())
        };
        if let Some(batch) = batch {
            play_batch(&app, &batch);
        }
    });
}

fn play_batch(app: &AppHandle, paths: &[String]) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let handle = state.player.handle();
    let dropped: Arc<DroppedFiles> = Arc::clone(&state.dropped);
    match play_files(&dropped, &handle, paths) {
        Ok(0) => {}
        Ok(_) => {
            if main_is_away(app) {
                if let Err(err) = mini::show(app, &Anchor::WorkAreaBottomRight) {
                    eprintln!("showing the mini player failed: {err}");
                }
            }
        }
        Err(err) => eprintln!("playing the opened files failed: {err}"),
    }
}

/// A second `prism.exe` was started: its files play here, with none the main window comes
/// forward.
pub fn on_second_instance(app: &AppHandle, args: &[String], cwd: &str) {
    let paths = paths_from_args(args, Path::new(cwd));
    if paths.is_empty() {
        if let Err(err) = crate::commands::main_show(app.clone()) {
            eprintln!("showing the main window failed: {err}");
        }
    } else {
        play_paths(app, paths);
    }
}

/// The files this instance was started with.
pub fn play_launch_args(app: &AppHandle) {
    let args: Vec<String> = std::env::args_os()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    if let Ok(cwd) = std::env::current_dir() {
        play_paths(app, paths_from_args(&args, &cwd));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{AudioSource, OutputEvent, OutputFactory, OutputSink, StreamSpec};
    use crate::library::model::Track;
    use crate::library::tags::fixtures;
    use crate::player::{with_dropped, EventSink, Opener, Player, PlayerOptions, Resolver};
    use crossbeam_channel::{Receiver, Sender};

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn args_keep_unc_and_spaced_paths() {
        let args = strings(&[
            "prism.exe",
            "--autostart",
            "\\\\nas\\music\\a b.flac",
            "rel\\x.mp3",
            "notes.txt",
        ]);
        assert_eq!(
            paths_from_args(&args, Path::new("C:\\Users\\me")),
            strings(&["\\\\nas\\music\\a b.flac", "C:\\Users\\me\\rel\\x.mp3"])
        );
    }

    #[test]
    fn args_skip_the_executable_and_flags() {
        let args = strings(&["song.mp3", "--x.flac", "b.FLAC"]);
        let cwd = std::env::temp_dir();
        assert_eq!(
            paths_from_args(&args, &cwd),
            vec![cwd.join("b.FLAC").to_string_lossy().into_owned()]
        );
    }

    #[test]
    fn file_urls_decode() {
        let urls = [
            tauri::Url::parse("file:///Users/me/A%20B.flac").unwrap(),
            tauri::Url::parse("file:///Users/me/notes.txt").unwrap(),
            tauri::Url::parse("https://example.com/a.flac").unwrap(),
            tauri::Url::parse("file:///C:/Music/%C3%A4.mp3").unwrap(),
        ];
        assert_eq!(
            paths_from_urls(&urls),
            strings(&[
                &PathBuf::from("/Users/me/A B.flac").to_string_lossy(),
                &PathBuf::from("C:/Music/\u{e4}.mp3").to_string_lossy(),
            ])
        );
    }

    #[test]
    fn forwards_within_the_window_play_once_in_order() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut queue = Debounce::default();
        assert!(
            queue.push(t0, strings(&["a.mp3"])),
            "the first push starts a worker"
        );
        assert!(!queue.push(t0 + ms(100), strings(&["b.mp3"])));
        assert!(!queue.push(t0 + ms(250), strings(&["c.mp3"])));
        assert_eq!(
            queue.take_due(t0 + ms(400)),
            None,
            "the window restarted at c"
        );
        assert_eq!(
            queue.take_due(t0 + ms(560)),
            Some(strings(&["a.mp3", "b.mp3", "c.mp3"]))
        );
        assert_eq!(
            queue.take_due(t0 + ms(2000)),
            None,
            "nothing is played twice"
        );
        // A forward after the window starts a new batch.
        queue.push(t0 + ms(3000), strings(&["d.mp3"]));
        assert_eq!(queue.take_due(t0 + ms(3400)), Some(strings(&["d.mp3"])));
    }

    #[test]
    fn a_steady_stream_is_cut_off_at_the_cap() {
        let t0 = Instant::now();
        let mut queue = Debounce::default();
        for i in 0..11u64 {
            queue.push(t0 + Duration::from_millis(i * 100), strings(&["x.mp3"]));
        }
        assert_eq!(
            queue
                .take_due(t0 + Duration::from_millis(1000))
                .map(|b| b.len()),
            Some(11)
        );
    }

    #[test]
    fn percent_decoding_keeps_stray_percent_signs() {
        assert_eq!(percent_decode("100%").as_deref(), Some("100%"));
        assert_eq!(percent_decode("a%2").as_deref(), Some("a%2"));
        assert_eq!(percent_decode("%zz%41").as_deref(), Some("%zzA"));
    }

    #[test]
    fn hooks_cover_every_audio_extension() {
        let hooks = include_str!("../../windows/hooks.nsh");
        let plist = include_str!("../../Info.plist");
        for ext in crate::library::scan::AUDIO_EXTENSIONS {
            assert!(hooks.contains(&format!(".{ext}")), "hooks.nsh lacks .{ext}");
            assert!(
                plist.contains(&format!("<string>{ext}</string>")),
                "Info.plist lacks {ext}"
            );
        }
        // Launch Services ignores the extensions once content types are listed.
        assert!(plist.contains("<string>public.audio</string>"));
        assert!(hooks.contains("NSIS_HOOK_POSTINSTALL"));
        assert!(hooks.contains("NSIS_HOOK_PREUNINSTALL"));
    }

    #[test]
    fn hooks_never_set_a_default() {
        let hooks = include_str!("../../windows/hooks.nsh");
        for line in hooks.lines() {
            let line = line.trim();
            if line.starts_with("WriteRegStr") && line.contains(r"Software\Classes\.") {
                // `WriteRegStr HKCU "<key>" "<value name>" "<value>"`: the value name must
                // be set, an empty one is the extension's default player.
                let name = line.split('"').nth(3).unwrap_or("");
                assert!(!name.is_empty(), "sets a default: {line}");
                assert!(
                    line.contains("OpenWithProgids") || line.contains("SupportedTypes"),
                    "writes outside the allowed keys: {line}"
                );
            }
        }
        let plist = include_str!("../../Info.plist");
        assert!(plist.contains("<string>Alternate</string>"));
        assert!(!plist.contains("Owner"));
        assert!(!plist.contains("Default"));
        assert_eq!(plist.matches("<key>CFBundleDocumentTypes</key>").count(), 1);
    }

    struct Quiet {
        events: Receiver<OutputEvent>,
        _keep: Sender<OutputEvent>,
    }

    const SPEC: StreamSpec = StreamSpec {
        sample_rate: 44_100,
        channels: 2,
    };

    impl OutputSink for Quiet {
        fn spec(&self) -> StreamSpec {
            SPEC
        }
        fn push(&mut self, _samples: &[f32]) -> usize {
            0
        }
        fn queued(&self) -> usize {
            0
        }
        fn clear(&mut self) {}
        fn set_volume(&mut self, _volume: f32) {}
        fn set_paused(&mut self, _paused: bool) {}
        fn errors(&self) -> Receiver<OutputEvent> {
            self.events.clone()
        }
    }

    struct QuietFactory;

    impl OutputFactory for QuietFactory {
        fn open(&self, _device: Option<String>) -> anyhow::Result<Box<dyn OutputSink>> {
            let (tx, rx) = crossbeam_channel::unbounded();
            Ok(Box::new(Quiet {
                events: rx,
                _keep: tx,
            }))
        }
    }

    struct Tone(u64);

    impl AudioSource for Tone {
        fn spec(&self) -> StreamSpec {
            SPEC
        }
        fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize> {
            let frames = self.0.min(1024);
            out.extend(std::iter::repeat_n(0.0, frames as usize * 2));
            self.0 -= frames;
            Ok(frames as usize)
        }
        fn seek(&mut self, _ms: u64) -> anyhow::Result<()> {
            Ok(())
        }
        fn duration_ms(&self) -> Option<u64> {
            Some(60_000)
        }
    }

    #[test]
    fn second_instance_plays_files() {
        let dropped = Arc::new(DroppedFiles::new(None));
        let resolver: Resolver = with_dropped(Arc::clone(&dropped), Arc::new(|_id: &str| None));
        let opener: Opener = Arc::new(|_track: &Track, _spec: StreamSpec| {
            Ok(Box::new(Tone(44_100 * 60)) as Box<dyn AudioSource>)
        });
        let sink: EventSink = Arc::new(|_event| {});
        let player = Player::spawn_with_options(
            Arc::new(QuietFactory),
            resolver,
            sink,
            opener,
            PlayerOptions::default(),
        );
        let fixture = fixtures().join("tagged.mp3");
        let args = strings(&[
            "prism.exe",
            "--autostart",
            &fixture.to_string_lossy(),
            "notes.txt",
        ]);
        // What the single-instance callback does with the forwarded argv.
        let paths = paths_from_args(&args, Path::new("."));
        let queued = play_files(&dropped, &player.handle(), &paths).unwrap();
        assert_eq!(queued, 1);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let ids = player.handle().queue_ids();
            if ids.len() == 1 {
                let track = dropped.get(&ids[0]).expect("the queued id resolves");
                assert_eq!(track.path.as_deref(), Some(&*fixture.to_string_lossy()));
                break;
            }
            assert!(std::time::Instant::now() < deadline, "queue stayed {ids:?}");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
