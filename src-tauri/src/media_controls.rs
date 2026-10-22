//! OS media controls: the Windows media overlay (SMTC) and macOS Now Playing, through
//! souvlaki. Player state flows in over a channel from the event sink; media keys flow back
//! as transport commands. A worker thread owns the controls, so cover resizing and OS calls
//! never run on the audio engine thread. If the controls cannot start, Prism runs without.
//!
//! The controls show whichever player is active (`shell::transport`): Prism's track, or
//! YouTube Music's song while it is active. The keys go through the transport to that player.
//! On Windows the YouTube Music view's own WebView2 entry is switched off by its browser
//! arguments, so Prism's is the only one; on macOS WKWebView may publish a Now Playing entry
//! of its own next to Prism's, and its public API has no switch for that.

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};
use tauri::{AppHandle, Manager};

use crate::library::model::Track;
use crate::player::{is_live, EventSink, PlayerEvent, PlayerState, PlayerStatus};
use crate::shell::transport::{Active, Command, Transport};
use crate::ytm::state::{YtmState, YtmStatus};
use crate::AppState;

/// Size of the cover handed to the OS.
const COVER_SIZE: u32 = 600;
/// Position updates reach the OS at most this often while playing.
const POSITION_INTERVAL: Duration = Duration::from_secs(1);
/// How far a seek key without an amount jumps.
const SEEK_STEP_MS: u64 = 10_000;

/// Track metadata in the shape the OS wants it, owning its strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// `file://` plus the cover path. souvlaki on Windows strips the prefix and opens the
    /// rest as a path, so the path is not percent-encoded.
    pub cover_url: Option<String>,
    /// `None` for a station. macOS then shows no progress bar; Windows (souvlaki sets the
    /// SMTC timeline to 0..0) shows an empty one.
    pub duration: Option<Duration>,
}

impl OwnedMetadata {
    pub fn as_metadata(&self) -> MediaMetadata<'_> {
        MediaMetadata {
            title: Some(&self.title),
            album: Some(&self.album),
            artist: Some(&self.artist),
            cover_url: self.cover_url.as_deref(),
            duration: self.duration,
        }
    }
}

/// The stream title that applies to `track`: only a station has one, and a blank one is none.
fn shown_title<'a>(track: &Track, live_title: Option<&'a str>) -> Option<&'a str> {
    live_title.filter(|title| is_live(track) && !title.trim().is_empty())
}

/// Maps a library track (and its cached cover file, if any) to OS metadata. A station shows
/// the stream title (`live_title`) over its name, or its name over its region line.
pub fn to_metadata(
    track: &Track,
    live_title: Option<&str>,
    cover_file: Option<&Path>,
) -> OwnedMetadata {
    let (title, artist) = match shown_title(track, live_title) {
        Some(title) => (title.to_string(), track.title.clone()),
        None => (track.title.clone(), track.artist.clone()),
    };
    OwnedMetadata {
        title,
        artist,
        album: track.album.clone(),
        cover_url: cover_file.map(|path| cover_url(path, cfg!(target_os = "windows"))),
        duration: (!is_live(track)).then(|| Duration::from_millis(track.duration_ms)),
    }
}

/// The `file://` URL of a cover. Windows (souvlaki strips the prefix and opens the rest as a
/// path) needs the raw path; elsewhere the OS parses a real URL, so the path is
/// percent-encoded.
pub fn cover_url(path: &Path, raw_path: bool) -> String {
    let text = path.to_string_lossy();
    if raw_path {
        return format!("file://{text}");
    }
    let mut url = String::from("file://");
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                url.push(byte as char)
            }
            other => url.push_str(&format!("%{other:02X}")),
        }
    }
    url
}

/// Clamps a seek target to the track; an unknown duration (0) clamps nothing.
fn clamp_seek(ms: u64, duration_ms: u64) -> u64 {
    if duration_ms == 0 {
        ms
    } else {
        ms.min(duration_ms)
    }
}

/// What a media key asks the player to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Toggle,
    Next,
    Prev,
    Seek(u64),
}

/// Maps an OS event to a player action, given the player's current state. The player only
/// has a toggle, so Play and Pause act only when they would change something.
///
/// A station cannot seek or skip. souvlaki 0.8 cannot disable single buttons: on Windows the
/// SMTC next and previous buttons stay enabled, on macOS the remote commands too, so on both
/// the keys are ignored here (and in the engine).
pub fn action_for(event: &MediaControlEvent, state: &PlayerState) -> Option<Action> {
    let live = state.track.as_ref().is_some_and(is_live);
    if live
        && matches!(
            event,
            MediaControlEvent::Next
                | MediaControlEvent::Previous
                | MediaControlEvent::SetPosition(_)
                | MediaControlEvent::Seek(_)
                | MediaControlEvent::SeekBy(..)
        )
    {
        return None;
    }
    let seek_by = |direction: SeekDirection, ms: u64| {
        let target = match direction {
            SeekDirection::Forward => state.position_ms.saturating_add(ms),
            SeekDirection::Backward => state.position_ms.saturating_sub(ms),
        };
        Some(Action::Seek(clamp_seek(target, state.duration_ms)))
    };
    match event {
        MediaControlEvent::Play => (state.status == PlayerStatus::Paused).then_some(Action::Toggle),
        MediaControlEvent::Pause | MediaControlEvent::Stop => {
            (state.status == PlayerStatus::Playing).then_some(Action::Toggle)
        }
        MediaControlEvent::Toggle => Some(Action::Toggle),
        MediaControlEvent::Next => Some(Action::Next),
        MediaControlEvent::Previous => Some(Action::Prev),
        MediaControlEvent::SetPosition(MediaPosition(position)) => Some(Action::Seek(clamp_seek(
            position.as_millis() as u64,
            state.duration_ms,
        ))),
        MediaControlEvent::Seek(direction) => seek_by(*direction, SEEK_STEP_MS),
        MediaControlEvent::SeekBy(direction, amount) => {
            seek_by(*direction, amount.as_millis() as u64)
        }
        _ => None,
    }
}

/// Maps an OS event to an action for YouTube Music's page, given its last state. Next and
/// previous act only when the page's buttons can be pressed, a seek only when the song's
/// length is known. As for a station, souvlaki 0.8 cannot disable single buttons (SMTC on
/// Windows, the remote commands on macOS), so the keys are ignored here.
pub fn ytm_action_for(event: &MediaControlEvent, state: &YtmState) -> Option<Action> {
    let seekable = state.duration_ms > 0;
    let seek_by = |direction: SeekDirection, ms: u64| {
        let target = match direction {
            SeekDirection::Forward => state.position_ms.saturating_add(ms),
            SeekDirection::Backward => state.position_ms.saturating_sub(ms),
        };
        seekable.then(|| Action::Seek(clamp_seek(target, state.duration_ms)))
    };
    match event {
        MediaControlEvent::Play => (state.status == YtmStatus::Paused).then_some(Action::Toggle),
        MediaControlEvent::Pause | MediaControlEvent::Stop => {
            (state.status == YtmStatus::Playing).then_some(Action::Toggle)
        }
        MediaControlEvent::Toggle => Some(Action::Toggle),
        MediaControlEvent::Next => state.can_next.then_some(Action::Next),
        MediaControlEvent::Previous => state.can_prev.then_some(Action::Prev),
        MediaControlEvent::SetPosition(MediaPosition(position)) => seekable
            .then(|| Action::Seek(clamp_seek(position.as_millis() as u64, state.duration_ms))),
        MediaControlEvent::Seek(direction) => seek_by(*direction, SEEK_STEP_MS),
        MediaControlEvent::SeekBy(direction, amount) => {
            seek_by(*direction, amount.as_millis() as u64)
        }
        _ => None,
    }
}

/// Maps an OS event for the active player, from that player's state.
pub fn media_action(
    event: &MediaControlEvent,
    active: Active,
    prism: &PlayerState,
    ytm: &YtmState,
) -> Option<Action> {
    match active {
        Active::Prism => action_for(event, prism),
        Active::Ytm => ytm_action_for(event, ytm),
    }
}

impl Action {
    fn command(self) -> Command {
        match self {
            Action::Toggle => Command::Toggle,
            Action::Next => Command::Next,
            Action::Prev => Command::Prev,
            Action::Seek(ms) => Command::Seek(ms),
        }
    }
}

/// Maps a media key for the active player and sends it through the transport to that same
/// player (the active player is read once). `state` gives each player's state when asked.
pub fn apply(
    transport: &Transport,
    event: &MediaControlEvent,
    state: impl FnOnce() -> (PlayerState, YtmState),
) -> Result<Option<Active>, String> {
    transport.route_with(|active| {
        let (prism, ytm) = state();
        media_action(event, active, &prism, &ytm).map(Action::command)
    })
}

/// The controls show YouTube Music while it is active and has a song loaded (an idle or
/// unavailable page hands the transport back to Prism anyway).
fn shows_ytm(active: Active, ytm: Option<&YtmState>) -> bool {
    active == Active::Ytm
        && ytm.is_some_and(|state| matches!(state.status, YtmStatus::Playing | YtmStatus::Paused))
}

/// Maps YouTube Music's state (and its cached artwork file, if any) to OS metadata. An
/// unknown length shows no progress, as for a station.
pub fn ytm_metadata(state: &YtmState, cover_file: Option<&Path>) -> OwnedMetadata {
    OwnedMetadata {
        title: state.title.clone(),
        artist: state.artist.clone(),
        album: state.album.clone(),
        cover_url: cover_file.map(|path| cover_url(path, cfg!(target_os = "windows"))),
        duration: (state.duration_ms > 0).then(|| Duration::from_millis(state.duration_ms)),
    }
}

fn ytm_playback(state: &YtmState) -> MediaPlayback {
    let progress =
        (state.duration_ms > 0).then(|| MediaPosition(Duration::from_millis(state.position_ms)));
    match state.status {
        YtmStatus::Playing => MediaPlayback::Playing { progress },
        YtmStatus::Paused => MediaPlayback::Paused { progress },
        YtmStatus::Idle | YtmStatus::Unavailable => MediaPlayback::Stopped,
    }
}

/// A station has no position to show: macOS then sets no elapsed time, Windows a position of 0.
fn playback_for(state: &PlayerState) -> MediaPlayback {
    let live = state.track.as_ref().is_some_and(is_live);
    let progress = (!live).then(|| MediaPosition(Duration::from_millis(state.position_ms)));
    match state.status {
        PlayerStatus::Idle => MediaPlayback::Stopped,
        PlayerStatus::Loading | PlayerStatus::Paused => MediaPlayback::Paused { progress },
        PlayerStatus::Playing => MediaPlayback::Playing { progress },
    }
}

/// A metadata update that failed, or went out without the track's own cover (an offline
/// server), is tried again after this long instead of on every push.
const RETRY_AFTER: Duration = Duration::from_secs(60);

/// What was last sent to the OS.
#[derive(Default)]
struct Pushed {
    track_id: Option<String>,
    /// The cover of `track_id` as sent; a lookup can add one to the playing track.
    cover_id: Option<String>,
    /// The stream title of a station as sent.
    title: Option<String>,
    status: Option<PlayerStatus>,
    position_ms: u64,
    at: Option<Instant>,
    /// The last `(track, cover)` whose update failed or lacked its own cover, and when.
    failed: Option<Failed>,
}

struct Failed {
    track_id: String,
    cover_id: Option<String>,
    at: Instant,
}

/// The cover file to send for a cover id, and whether that is the own cover (`false` when an
/// id got the placeholder).
type CoverFor = Box<dyn Fn(Option<&str>) -> (Option<PathBuf>, bool)>;

/// The position goes out while playing at most every [`POSITION_INTERVAL`]; otherwise when it
/// moved (a seek while paused must show up too).
fn position_due(playing: bool, at: Option<Instant>, pushed_ms: u64, position_ms: u64) -> bool {
    if playing {
        at.is_none_or(|at| at.elapsed() >= POSITION_INTERVAL)
    } else {
        position_ms != pushed_ms
    }
}

/// What the OS shows of a YouTube Music song; the position is not part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct YtmShown {
    title: String,
    artist: String,
    album: String,
    cover_id: Option<String>,
    duration_ms: u64,
}

impl YtmShown {
    fn of(state: &YtmState) -> YtmShown {
        YtmShown {
            title: state.title.clone(),
            artist: state.artist.clone(),
            album: state.album.clone(),
            cover_id: state.cover_id.clone(),
            duration_ms: state.duration_ms,
        }
    }
}

/// What was last sent to the OS for YouTube Music.
#[derive(Default)]
struct PushedYtm {
    shown: Option<YtmShown>,
    /// The song whose update failed or went out without its artwork, and when.
    failed: Option<(YtmShown, Instant)>,
    status: Option<YtmStatus>,
    position_ms: u64,
    at: Option<Instant>,
}

/// As [`metadata_due`]: a new song, title or artwork goes out at once; one that failed (or
/// lacked its artwork) waits [`RETRY_AFTER`].
fn ytm_metadata_due(pushed: &PushedYtm, shown: &YtmShown, now: Instant) -> bool {
    match &pushed.failed {
        Some((failed, at)) if failed == shown => now.saturating_duration_since(*at) >= RETRY_AFTER,
        _ => pushed.shown.as_ref() != Some(shown),
    }
}

fn push_ytm(
    controls: &mut MediaControls,
    pushed: &mut PushedYtm,
    state: &YtmState,
    cover_for: &CoverFor,
) {
    let now = Instant::now();
    let shown = YtmShown::of(state);
    let metadata_changed = ytm_metadata_due(pushed, &shown, now);
    let status_changed = Some(state.status) != pushed.status;
    let position_due = state.duration_ms > 0
        && position_due(
            state.status == YtmStatus::Playing,
            pushed.at,
            pushed.position_ms,
            state.position_ms,
        );
    if !(metadata_changed || status_changed || position_due) {
        return;
    }
    if metadata_changed {
        let (cover, own) = cover_for(state.cover_id.as_deref());
        let metadata = ytm_metadata(state, cover.as_deref());
        match controls.set_metadata(metadata.as_metadata()) {
            Ok(()) => {
                pushed.failed = (!own).then(|| (shown.clone(), now));
                pushed.shown = Some(shown);
            }
            Err(err) => {
                eprintln!("updating the media metadata failed: {err:?}");
                pushed.failed = Some((shown, now));
            }
        }
    }
    if let Err(err) = controls.set_playback(ytm_playback(state)) {
        eprintln!("updating the media playback state failed: {err:?}");
    }
    pushed.status = Some(state.status);
    pushed.position_ms = state.position_ms;
    pushed.at = Some(Instant::now());
}

/// Prism is shown again without a track: the last song's metadata goes.
fn clear(controls: &mut MediaControls) {
    if let Err(err) = controls.set_metadata(MediaMetadata::default()) {
        eprintln!("clearing the media metadata failed: {err:?}");
    }
    if let Err(err) = controls.set_playback(MediaPlayback::Stopped) {
        eprintln!("updating the media playback state failed: {err:?}");
    }
}

/// The OS metadata is out of date: another track, or the same one with another cover.
fn metadata_stale(pushed: &Pushed, track: Option<&Track>) -> bool {
    track.map(|t| t.id.as_str()) != pushed.track_id.as_deref()
        || track.is_some_and(|t| t.cover_id != pushed.cover_id)
}

/// The metadata is pushed now: it is out of date, except that a `(track, cover)` that failed
/// within the last [`RETRY_AFTER`] waits, and one that went out without its own cover is
/// tried again once that time has passed. A station's new stream title goes out at once.
fn metadata_due(
    pushed: &Pushed,
    track: Option<&Track>,
    live_title: Option<&str>,
    now: Instant,
) -> bool {
    let title = track.and_then(|t| shown_title(t, live_title));
    if track.is_some() && title != pushed.title.as_deref() {
        return true;
    }
    let failed = track.and_then(|t| {
        pushed
            .failed
            .as_ref()
            .filter(|f| f.track_id == t.id && f.cover_id == t.cover_id)
    });
    match failed {
        Some(failed) => now.saturating_duration_since(failed.at) >= RETRY_AFTER,
        None => metadata_stale(pushed, track),
    }
}

fn failed_now(track: &Track, now: Instant) -> Option<Failed> {
    Some(Failed {
        track_id: track.id.clone(),
        cover_id: track.cover_id.clone(),
        at: now,
    })
}

fn push(
    controls: &mut MediaControls,
    pushed: &mut Pushed,
    state: &PlayerState,
    live_title: Option<&str>,
    cover_for: &CoverFor,
) {
    let now = Instant::now();
    let track_changed = metadata_due(pushed, state.track.as_ref(), live_title, now);
    let status_changed = Some(state.status) != pushed.status;
    let live = state.track.as_ref().is_some_and(is_live);
    let position_due = !live
        && position_due(
            state.status == PlayerStatus::Playing,
            pushed.at,
            pushed.position_ms,
            state.position_ms,
        );
    if !(track_changed || status_changed || position_due) {
        return;
    }
    if track_changed {
        match &state.track {
            Some(track) => {
                let (cover, own) = cover_for(track.cover_id.as_deref());
                let metadata = to_metadata(track, live_title, cover.as_deref());
                // Set even when the update fails, so a title is not resent on every push.
                pushed.title = shown_title(track, live_title).map(str::to_string);
                match controls.set_metadata(metadata.as_metadata()) {
                    // A missing own cover is fetched again in a minute.
                    Ok(()) => {
                        pushed.track_id = Some(track.id.clone());
                        pushed.cover_id = track.cover_id.clone();
                        pushed.failed = if own { None } else { failed_now(track, now) };
                    }
                    // Retried in a minute, not on every push.
                    Err(err) => {
                        eprintln!("updating the media metadata failed: {err:?}");
                        pushed.failed = failed_now(track, now);
                    }
                }
            }
            None => {
                pushed.track_id = None;
                pushed.cover_id = None;
                pushed.title = None;
                pushed.failed = None;
            }
        }
    }
    if let Err(err) = controls.set_playback(playback_for(state)) {
        eprintln!("updating the media playback state failed: {err:?}");
    }
    pushed.status = Some(state.status);
    pushed.position_ms = state.position_ms;
    pushed.at = Some(Instant::now());
}

/// The player-state side of the OS controls. [`sink`](MediaBridge::sink) goes into the
/// player's event fan-out; [`start`](MediaBridge::start) runs once the main window exists.
pub struct MediaBridge {
    updates: Sender<Update>,
    pending: Mutex<Option<Receiver<Update>>>,
}

/// What reaches the worker.
enum Update {
    State(Box<PlayerState>),
    /// A station's stream title (`None`: none known, or the station was opened again).
    LiveTitle(String, Option<String>),
    Ytm(Box<YtmState>),
    Active(Active),
}

/// The last stream title and the station it belongs to. It is shown only while that station
/// is the shown track; showing another track drops it, so a title never moves to the next
/// station. A title may arrive before the state that shows its station.
#[derive(Default)]
struct LiveTitle {
    station_id: Option<String>,
    title: Option<String>,
    /// The shown track id last seen.
    shown: Option<String>,
}

impl LiveTitle {
    fn set(&mut self, station_id: String, title: Option<String>) {
        self.station_id = Some(station_id);
        self.title = title;
    }

    /// The title for the shown `track`, if it is the title's station.
    fn for_shown(&mut self, track: Option<&Track>) -> Option<&str> {
        let shown = track.map(|t| t.id.clone());
        if shown != self.shown {
            if self.station_id != shown {
                self.station_id = None;
                self.title = None;
            }
            self.shown = shown;
        }
        if self.station_id.is_some() && self.station_id == self.shown {
            self.title.as_deref()
        } else {
            None
        }
    }
}

impl MediaBridge {
    pub fn new() -> Arc<MediaBridge> {
        let (updates, receiver) = crossbeam_channel::unbounded();
        Arc::new(MediaBridge {
            updates,
            pending: Mutex::new(Some(receiver)),
        })
    }

    /// Forwards state events to the worker; never blocks.
    pub fn sink(self: &Arc<Self>) -> EventSink {
        let bridge = Arc::clone(self);
        Arc::new(move |event| {
            if let PlayerEvent::State(state) = event {
                let _ = bridge.updates.send(Update::State(Box::new(state)));
            }
        })
    }

    /// The stream title of `station_id`, shown in place of its name while that station is the
    /// shown track (ignored otherwise; another track drops it). Never blocks.
    pub fn set_live_title(&self, station_id: String, title: Option<String>) {
        let _ = self.updates.send(Update::LiveTitle(station_id, title));
    }

    /// YouTube Music's new state (`ytm://state`). Never blocks.
    pub fn set_ytm(&self, state: &YtmState) {
        let _ = self.updates.send(Update::Ytm(Box::new(state.clone())));
    }

    /// The transport's new active player (`transport://active`). Never blocks.
    pub fn set_active(&self, active: Active) {
        let _ = self.updates.send(Update::Active(active));
    }

    /// Starts the worker. `hwnd` is the main window handle (Windows) or `None`. The worker
    /// keeps draining states even when the OS controls fail to initialise.
    pub fn start(&self, app: AppHandle, hwnd: Option<usize>, cover_cache: PathBuf) {
        let receiver = match self.pending.lock() {
            Ok(mut pending) => pending.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        let Some(receiver) = receiver else { return };
        let spawned = std::thread::Builder::new()
            .name("prism-media-controls".into())
            .spawn(move || {
                let run = catch_unwind(AssertUnwindSafe(|| run(receiver, app, hwnd, cover_cache)));
                if run.is_err() {
                    eprintln!("the media controls stopped after a panic");
                }
            });
        if let Err(err) = spawned {
            eprintln!("starting the media controls failed: {err}");
        }
    }
}

fn create(app: &AppHandle, hwnd: Option<usize>) -> Option<MediaControls> {
    #[cfg(target_os = "windows")]
    if hwnd.is_none() {
        eprintln!("media controls need a window handle; running without them");
        return None;
    }
    let config = PlatformConfig {
        display_name: "Prism",
        dbus_name: "prism",
        hwnd: hwnd.map(|handle| handle as *mut c_void),
    };
    let created = catch_unwind(AssertUnwindSafe(|| MediaControls::new(config)));
    let mut controls = match created {
        Ok(Ok(controls)) => controls,
        Ok(Err(err)) => {
            eprintln!("media controls are unavailable: {err:?}");
            return None;
        }
        Err(_) => {
            eprintln!("media controls are unavailable: initialisation panicked");
            return None;
        }
    };
    let handle = app.clone();
    let attached = controls.attach(move |event| {
        // A panic must not unwind into the OS callback.
        let handled = catch_unwind(AssertUnwindSafe(|| {
            let Some(state) = handle.try_state::<AppState>() else {
                return;
            };
            let states = || (state.player.state(), state.ytm.state());
            if let Err(err) = apply(&state.transport, &event, states) {
                eprintln!("a media key could not be applied: {err}");
            }
        }));
        if handled.is_err() {
            eprintln!("a media key panicked");
        }
    });
    match attached {
        Ok(()) => Some(controls),
        Err(err) => {
            eprintln!("media controls could not attach: {err:?}");
            None
        }
    }
}

fn run(receiver: Receiver<Update>, app: AppHandle, hwnd: Option<usize>, cover_cache: PathBuf) {
    let mut controls = create(&app, hwnd);
    let cover_app = app.clone();
    // `yt-` artwork comes through the same gate as Prism's covers.
    let cover_for: CoverFor = Box::new(move |id| {
        let own = || {
            let id = id?;
            let state = cover_app.try_state::<AppState>()?;
            state
                .thumbs
                .get(&state.library.covers, &cover_cache, id, COVER_SIZE)
                .ok()
        };
        // The overlay keeps the previous artwork unless a new one is set, so a track
        // without a usable cover gets the placeholder.
        match own() {
            Some(path) => (Some(path), true),
            None => (crate::thumbs::placeholder_cover(&cover_cache), id.is_none()),
        }
    });
    let mut pushed = Pushed::default();
    let mut pushed_ytm = PushedYtm::default();
    let mut state: Option<Box<PlayerState>> = None;
    let mut ytm: Option<Box<YtmState>> = None;
    let mut active = Active::Prism;
    let mut showing_ytm = false;
    let mut titles = LiveTitle::default();
    while let Ok(update) = receiver.recv() {
        // Only the newest states and title matter.
        for update in std::iter::once(update).chain(receiver.try_iter()) {
            match update {
                Update::State(newer) => state = Some(newer),
                Update::LiveTitle(station_id, title) => titles.set(station_id, title),
                Update::Ytm(newer) => ytm = Some(newer),
                Update::Active(newer) => active = newer,
            }
        }
        let Some(controls) = controls.as_mut() else {
            continue;
        };
        let show_ytm = shows_ytm(active, ytm.as_deref());
        // The other player's last push says nothing about what the OS shows now.
        let switched = show_ytm != showing_ytm;
        if switched {
            pushed = Pushed::default();
            pushed_ytm = PushedYtm::default();
            showing_ytm = show_ytm;
        }
        if let Some(ytm) = ytm.as_deref().filter(|_| show_ytm) {
            push_ytm(controls, &mut pushed_ytm, ytm, &cover_for);
            continue;
        }
        if switched && state.as_ref().is_none_or(|state| state.track.is_none()) {
            clear(controls);
        }
        if let Some(state) = state.as_ref() {
            let title = titles.for_shown(state.track.as_ref());
            push(controls, &mut pushed, state, title, &cover_for);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::model::Source;
    use crate::settings::RepeatMode;

    fn track() -> Track {
        Track {
            id: "t1".into(),
            source: Source::Local,
            title: "Song".into(),
            artist: "Artist".into(),
            album_artist: "Album Artist".into(),
            album: "Album".into(),
            album_id: "a1".into(),
            track_no: Some(3),
            disc_no: None,
            duration_ms: 215_500,
            year: None,
            genre: None,
            format: "flac".into(),
            sample_rate: None,
            bit_depth: None,
            bitrate_kbps: None,
            cover_id: Some("c1".into()),
            path: Some("song.flac".into()),
            replay_gain_track_db: None,
            replay_gain_album_db: None,
            replay_gain_track_peak: None,
            replay_gain_album_peak: None,
        }
    }

    #[test]
    fn metadata_is_stale_for_another_track_or_another_cover() {
        let mut pushed = Pushed::default();
        let playing = track();
        assert!(metadata_stale(&pushed, Some(&playing)), "nothing sent yet");
        assert!(!metadata_stale(&pushed, None));

        pushed.track_id = Some(playing.id.clone());
        pushed.cover_id = playing.cover_id.clone();
        assert!(!metadata_stale(&pushed, Some(&playing)));

        let mut other = track();
        other.id = "t2".into();
        assert!(metadata_stale(&pushed, Some(&other)));

        let mut covered = track();
        covered.cover_id = Some("found-online".into());
        assert!(
            metadata_stale(&pushed, Some(&covered)),
            "a new cover re-pushes"
        );
        covered.cover_id = None;
        assert!(metadata_stale(&pushed, Some(&covered)));

        assert!(metadata_stale(&pushed, None), "track gone");
    }

    #[test]
    fn a_failed_track_and_cover_is_retried_after_a_minute() {
        let start = Instant::now();
        let mut playing = track();
        playing.cover_id = Some("nd-al-1".into());
        let mut pushed = Pushed {
            failed: failed_now(&playing, start),
            ..Pushed::default()
        };
        // The update failed: not again on the next pushes.
        let due = |pushed: &Pushed, track: &Track, at: Instant| {
            metadata_due(pushed, Some(track), None, at)
        };
        assert!(!due(&pushed, &playing, start));
        assert!(!due(&pushed, &playing, start + Duration::from_secs(59)));
        assert!(due(&pushed, &playing, start + RETRY_AFTER));
        // Another track, or another cover, goes out at once.
        let mut other = track();
        other.id = "t2".into();
        assert!(due(&pushed, &other, start));
        let mut recovered = playing.clone();
        recovered.cover_id = Some("nd-al-2".into());
        assert!(due(&pushed, &recovered, start));

        // Sent with the placeholder: up to date, but the own cover is tried in a minute.
        pushed.track_id = Some(playing.id.clone());
        pushed.cover_id = playing.cover_id.clone();
        assert!(!due(&pushed, &playing, start));
        assert!(due(&pushed, &playing, start + RETRY_AFTER));
        // Sent with its own cover: nothing to retry.
        pushed.failed = None;
        assert!(!due(&pushed, &playing, start + RETRY_AFTER));
    }

    fn state(status: PlayerStatus, position_ms: u64) -> PlayerState {
        PlayerState {
            status,
            track: Some(track()),
            position_ms,
            duration_ms: 215_500,
            volume: 0.8,
            queue_index: 0,
            queue_length: 1,
            queue_version: 0,
            flow: None,
            repeat: RepeatMode::Off,
            shuffle: false,
        }
    }

    #[test]
    fn maps_track_to_metadata() {
        let cover = Path::new("cache").join("covers").join("600").join("c1.jpg");
        let metadata = to_metadata(&track(), None, Some(&cover));
        assert_eq!(metadata.title, "Song");
        assert_eq!(metadata.artist, "Artist");
        assert_eq!(metadata.album, "Album");
        assert_eq!(metadata.duration, Some(Duration::from_millis(215_500)));
        assert_eq!(
            metadata.cover_url,
            Some(cover_url(&cover, cfg!(target_os = "windows")))
        );
        let borrowed = metadata.as_metadata();
        assert_eq!(borrowed.title, Some("Song"));
        assert_eq!(borrowed.duration, Some(Duration::from_millis(215_500)));
    }

    #[test]
    fn cover_url_is_raw_for_windows_and_encoded_elsewhere() {
        let windows = Path::new(r"C:\Users\Jo Doe\cache\c#1.jpg");
        assert_eq!(
            cover_url(windows, true),
            r"file://C:\Users\Jo Doe\cache\c#1.jpg"
        );
        let mac = Path::new("/Users/Jo Doe/Library/Caches/c#1 (ü).jpg");
        assert_eq!(
            cover_url(mac, false),
            "file:///Users/Jo%20Doe/Library/Caches/c%231%20%28%C3%BC%29.jpg"
        );
    }

    #[test]
    fn seeks_clamp_to_a_known_duration_only() {
        let mut playing = state(PlayerStatus::Playing, 60_000);
        playing.duration_ms = 0;
        let seek = MediaControlEvent::SetPosition(MediaPosition(Duration::from_secs(90)));
        assert_eq!(action_for(&seek, &playing), Some(Action::Seek(90_000)));
        assert_eq!(
            action_for(&MediaControlEvent::Seek(SeekDirection::Forward), &playing),
            Some(Action::Seek(70_000))
        );
    }

    #[test]
    fn no_cover_means_no_cover_url() {
        assert_eq!(to_metadata(&track(), None, None).cover_url, None);
    }

    #[test]
    fn play_and_pause_only_act_when_they_change_something() {
        let playing = state(PlayerStatus::Playing, 0);
        let paused = state(PlayerStatus::Paused, 0);
        assert_eq!(
            action_for(&MediaControlEvent::Play, &paused),
            Some(Action::Toggle)
        );
        assert_eq!(action_for(&MediaControlEvent::Play, &playing), None);
        assert_eq!(
            action_for(&MediaControlEvent::Pause, &playing),
            Some(Action::Toggle)
        );
        assert_eq!(action_for(&MediaControlEvent::Pause, &paused), None);
        assert_eq!(
            action_for(&MediaControlEvent::Stop, &playing),
            Some(Action::Toggle)
        );
        assert_eq!(
            action_for(&MediaControlEvent::Toggle, &paused),
            Some(Action::Toggle)
        );
    }

    #[test]
    fn skips_and_seeks_map_to_actions() {
        let playing = state(PlayerStatus::Playing, 60_000);
        assert_eq!(
            action_for(&MediaControlEvent::Next, &playing),
            Some(Action::Next)
        );
        assert_eq!(
            action_for(&MediaControlEvent::Previous, &playing),
            Some(Action::Prev)
        );
        assert_eq!(
            action_for(
                &MediaControlEvent::SetPosition(MediaPosition(Duration::from_secs(90))),
                &playing
            ),
            Some(Action::Seek(90_000))
        );
        assert_eq!(
            action_for(&MediaControlEvent::Seek(SeekDirection::Forward), &playing),
            Some(Action::Seek(70_000))
        );
        assert_eq!(
            action_for(
                &MediaControlEvent::SeekBy(SeekDirection::Backward, Duration::from_secs(100)),
                &playing
            ),
            Some(Action::Seek(0))
        );
        assert_eq!(
            action_for(
                &MediaControlEvent::SetPosition(MediaPosition(Duration::from_secs(9999))),
                &playing
            ),
            Some(Action::Seek(215_500))
        );
        assert_eq!(action_for(&MediaControlEvent::Raise, &playing), None);
    }

    fn station() -> Track {
        crate::radio::model::tests::station(1).as_track()
    }

    fn station_state(status: PlayerStatus, position_ms: u64) -> PlayerState {
        PlayerState {
            track: Some(station()),
            duration_ms: 0,
            ..state(status, position_ms)
        }
    }

    #[test]
    fn live_metadata_shows_title_and_station() {
        let station = station();
        let cover = Path::new("cache")
            .join("covers")
            .join("600")
            .join("rb-1.jpg");
        let titled = to_metadata(&station, Some("Demo Artist - Demo Song"), Some(&cover));
        assert_eq!(titled.title, "Demo Artist - Demo Song");
        assert_eq!(titled.artist, "Demo Radio 1", "the station under the title");
        assert_eq!(titled.album, "");
        assert_eq!(titled.duration, None, "a station has no length");
        assert_eq!(titled.as_metadata().duration, None);
        assert_eq!(
            titled.cover_url,
            Some(cover_url(&cover, cfg!(target_os = "windows")))
        );
        let untitled = to_metadata(&station, None, None);
        assert_eq!(untitled.title, "Demo Radio 1");
        assert!(!station.artist.is_empty());
        assert_eq!(untitled.artist, station.artist, "the region line");
        assert_eq!(
            to_metadata(&station, Some("  "), None).title,
            "Demo Radio 1"
        );
        // A stream title never replaces a track's own.
        let song = to_metadata(&track(), Some("Demo Artist - Demo Song"), None);
        assert_eq!(song.title, "Song");
        assert_eq!(song.duration, Some(Duration::from_millis(215_500)));
    }

    #[test]
    fn live_disables_seek_next_and_prev() {
        for status in [PlayerStatus::Playing, PlayerStatus::Paused] {
            let live = station_state(status, 5_000);
            for event in [
                MediaControlEvent::Next,
                MediaControlEvent::Previous,
                MediaControlEvent::SetPosition(MediaPosition(Duration::from_secs(9))),
                MediaControlEvent::Seek(SeekDirection::Forward),
                MediaControlEvent::SeekBy(SeekDirection::Backward, Duration::from_secs(5)),
            ] {
                assert_eq!(action_for(&event, &live), None, "{status:?} {event:?}");
            }
            assert_eq!(
                action_for(&MediaControlEvent::Toggle, &live),
                Some(Action::Toggle)
            );
        }
        assert_eq!(
            action_for(
                &MediaControlEvent::Play,
                &station_state(PlayerStatus::Paused, 0)
            ),
            Some(Action::Toggle),
            "Play joins the station again"
        );
    }

    #[test]
    fn live_playback_has_no_position() {
        assert_eq!(
            playback_for(&station_state(PlayerStatus::Playing, 5_000)),
            MediaPlayback::Playing { progress: None }
        );
        assert_eq!(
            playback_for(&station_state(PlayerStatus::Paused, 0)),
            MediaPlayback::Paused { progress: None }
        );
    }

    #[test]
    fn a_stream_title_belongs_to_its_station() {
        let one = station();
        let two = crate::radio::model::tests::station(2).as_track();
        let song = track();
        let mut titles = LiveTitle::default();
        assert_eq!(titles.for_shown(Some(&one)), None);
        titles.set(one.id.clone(), Some("A".into()));
        assert_eq!(titles.for_shown(Some(&one)), Some("A"));
        // A late title of another station is not shown on this one.
        titles.set(two.id.clone(), Some("B".into()));
        assert_eq!(titles.for_shown(Some(&one)), None);
        // It shows once its station is shown (the title came before the state).
        assert_eq!(titles.for_shown(Some(&two)), Some("B"));
        // A new track drops it, also when the station comes back.
        assert_eq!(titles.for_shown(Some(&song)), None);
        assert_eq!(titles.for_shown(Some(&two)), None);
        titles.set(two.id.clone(), Some("C".into()));
        assert_eq!(titles.for_shown(Some(&two)), Some("C"));
        titles.set(two.id.clone(), None);
        assert_eq!(titles.for_shown(Some(&two)), None, "reset on open");
        titles.set(two.id.clone(), Some("D".into()));
        assert_eq!(titles.for_shown(None), None, "idle drops it");
        assert_eq!(titles.for_shown(Some(&two)), None);
    }

    #[test]
    fn a_new_stream_title_pushes_the_metadata_again() {
        let now = Instant::now();
        let station = station();
        let mut pushed = Pushed {
            track_id: Some(station.id.clone()),
            cover_id: station.cover_id.clone(),
            ..Pushed::default()
        };
        assert!(!metadata_due(&pushed, Some(&station), None, now));
        assert!(metadata_due(&pushed, Some(&station), Some("A"), now));
        pushed.title = Some("A".into());
        assert!(!metadata_due(&pushed, Some(&station), Some("A"), now));
        assert!(metadata_due(&pushed, Some(&station), Some("B"), now));
        assert!(
            metadata_due(&pushed, Some(&station), None, now),
            "title gone"
        );
        // A title only counts for a station.
        let song = track();
        let pushed = Pushed {
            track_id: Some(song.id.clone()),
            cover_id: song.cover_id.clone(),
            ..Pushed::default()
        };
        assert!(!metadata_due(&pushed, Some(&song), Some("A"), now));
    }

    fn ytm(status: YtmStatus) -> YtmState {
        YtmState {
            status,
            title: "Fake Song".into(),
            artist: "Fake Artist".into(),
            album: "Fake Album".into(),
            cover_id: Some("yt-0123456789abcdef0123".into()),
            position_ms: 30_000,
            duration_ms: 180_000,
            can_next: true,
            can_prev: true,
        }
    }

    #[test]
    fn media_metadata_follows_the_active_player() {
        let prism = state(PlayerStatus::Paused, 1_000);
        let page = ytm(YtmStatus::Playing);
        // Prism's track while Prism is active, YouTube Music's while it is.
        assert!(!shows_ytm(Active::Prism, Some(&page)));
        assert!(shows_ytm(Active::Ytm, Some(&page)));
        assert!(shows_ytm(Active::Ytm, Some(&ytm(YtmStatus::Paused))));
        assert!(!shows_ytm(Active::Ytm, None), "nothing reported yet");
        for dead in [YtmStatus::Idle, YtmStatus::Unavailable] {
            assert!(!shows_ytm(Active::Ytm, Some(&ytm(dead))), "{dead:?}");
        }
        assert_eq!(prism.track.as_ref().unwrap().title, "Song");

        let cover = Path::new("cache")
            .join("covers")
            .join("600")
            .join("yt-0123456789abcdef0123.jpg");
        let metadata = ytm_metadata(&page, Some(&cover));
        assert_eq!(metadata.title, "Fake Song");
        assert_eq!(metadata.artist, "Fake Artist");
        assert_eq!(metadata.album, "Fake Album");
        assert_eq!(metadata.duration, Some(Duration::from_millis(180_000)));
        assert_eq!(
            metadata.cover_url,
            Some(cover_url(&cover, cfg!(target_os = "windows")))
        );
        assert_eq!(
            ytm_playback(&page),
            MediaPlayback::Playing {
                progress: Some(MediaPosition(Duration::from_millis(30_000)))
            }
        );
        assert_eq!(
            ytm_playback(&ytm(YtmStatus::Paused)),
            MediaPlayback::Paused {
                progress: Some(MediaPosition(Duration::from_millis(30_000)))
            }
        );
        assert_eq!(ytm_playback(&ytm(YtmStatus::Idle)), MediaPlayback::Stopped);

        // An unknown length: no progress bar, as for a station.
        let unknown = YtmState {
            duration_ms: 0,
            ..ytm(YtmStatus::Playing)
        };
        assert_eq!(ytm_metadata(&unknown, None).duration, None);
        assert_eq!(ytm_metadata(&unknown, None).cover_url, None);
        assert_eq!(
            ytm_playback(&unknown),
            MediaPlayback::Playing { progress: None }
        );
    }

    #[test]
    fn ytm_keys_follow_its_buttons_and_its_length() {
        let playing = ytm(YtmStatus::Playing);
        let paused = ytm(YtmStatus::Paused);
        let key = |event: MediaControlEvent, state: &YtmState| {
            media_action(
                &event,
                Active::Ytm,
                &self::state(PlayerStatus::Idle, 0),
                state,
            )
        };
        assert_eq!(
            key(MediaControlEvent::Toggle, &playing),
            Some(Action::Toggle)
        );
        assert_eq!(key(MediaControlEvent::Play, &paused), Some(Action::Toggle));
        assert_eq!(key(MediaControlEvent::Play, &playing), None);
        assert_eq!(
            key(MediaControlEvent::Pause, &playing),
            Some(Action::Toggle)
        );
        assert_eq!(key(MediaControlEvent::Pause, &paused), None);
        assert_eq!(key(MediaControlEvent::Next, &playing), Some(Action::Next));
        assert_eq!(
            key(MediaControlEvent::Previous, &playing),
            Some(Action::Prev)
        );
        assert_eq!(
            key(
                MediaControlEvent::SetPosition(MediaPosition(Duration::from_secs(999))),
                &playing
            ),
            Some(Action::Seek(180_000))
        );
        assert_eq!(
            key(MediaControlEvent::Seek(SeekDirection::Forward), &playing),
            Some(Action::Seek(40_000))
        );
        assert_eq!(
            key(
                MediaControlEvent::SeekBy(SeekDirection::Backward, Duration::from_secs(60)),
                &playing
            ),
            Some(Action::Seek(0))
        );
        // Buttons the page cannot press, and seeks without a known length, do nothing.
        let stuck = YtmState {
            can_next: false,
            can_prev: false,
            duration_ms: 0,
            ..playing.clone()
        };
        for event in [
            MediaControlEvent::Next,
            MediaControlEvent::Previous,
            MediaControlEvent::SetPosition(MediaPosition(Duration::from_secs(9))),
            MediaControlEvent::Seek(SeekDirection::Forward),
            MediaControlEvent::SeekBy(SeekDirection::Forward, Duration::from_secs(5)),
        ] {
            assert_eq!(key(event.clone(), &stuck), None, "{event:?}");
        }
        assert_eq!(key(MediaControlEvent::Raise, &playing), None);
        // While Prism is active, Prism's state decides.
        assert_eq!(
            media_action(
                &MediaControlEvent::Play,
                Active::Prism,
                &state(PlayerStatus::Paused, 0),
                &playing
            ),
            Some(Action::Toggle)
        );
    }

    #[test]
    fn media_keys_go_through_the_transport() {
        use crate::shell::transport::tests::{Call, Rig};
        use crate::ytm::state::YtmAction;

        let rig = Rig::new();
        let prism = state(PlayerStatus::Playing, 1_000);
        let page = ytm(YtmStatus::Playing);
        let states = || (prism.clone(), page.clone());
        let at = |ms| MediaControlEvent::SetPosition(MediaPosition(Duration::from_millis(ms)));
        let press = |event: MediaControlEvent| apply(&rig.transport, &event, states);
        assert_eq!(press(MediaControlEvent::Toggle), Ok(Some(Active::Prism)));
        assert_eq!(press(at(5_000)), Ok(Some(Active::Prism)));
        assert_eq!(rig.prism.calls(), vec![Call::Toggle, Call::Seek(5_000)]);
        rig.ytm_plays();
        for event in [
            MediaControlEvent::Toggle,
            MediaControlEvent::Next,
            MediaControlEvent::Previous,
            at(7_000),
        ] {
            assert_eq!(press(event), Ok(Some(Active::Ytm)));
        }
        assert_eq!(
            rig.ytm.sent(),
            vec![
                YtmAction::Toggle,
                YtmAction::Next,
                YtmAction::Prev,
                YtmAction::Seek(7_000)
            ]
        );
        assert_eq!(rig.prism.calls().len(), 2);

        // Mapped from the page's state: a Play while it plays does nothing.
        assert_eq!(press(MediaControlEvent::Play), Ok(None));
        assert_eq!(rig.ytm.sent().len(), 4);
    }

    #[test]
    fn ytm_metadata_goes_out_on_a_change_or_a_minute_after_a_failure() {
        let start = Instant::now();
        let page = ytm(YtmStatus::Playing);
        let shown = YtmShown::of(&page);
        let mut pushed = PushedYtm::default();
        assert!(ytm_metadata_due(&pushed, &shown, start), "nothing sent yet");
        pushed.shown = Some(shown.clone());
        assert!(!ytm_metadata_due(&pushed, &shown, start));
        // The position is not part of it.
        let later = YtmShown::of(&YtmState {
            position_ms: 90_000,
            ..page.clone()
        });
        assert!(!ytm_metadata_due(&pushed, &later, start));
        for changed in [
            YtmState {
                title: "Other".into(),
                ..page.clone()
            },
            YtmState {
                cover_id: None,
                ..page.clone()
            },
            YtmState {
                duration_ms: 1_000,
                ..page.clone()
            },
        ] {
            assert!(ytm_metadata_due(&pushed, &YtmShown::of(&changed), start));
        }
        // Failed, or sent without its artwork: again after a minute, not before.
        pushed.failed = Some((shown.clone(), start));
        assert!(!ytm_metadata_due(
            &pushed,
            &shown,
            start + Duration::from_secs(59)
        ));
        assert!(ytm_metadata_due(&pushed, &shown, start + RETRY_AFTER));
    }

    #[test]
    fn playback_follows_the_player_status() {
        let progress = Some(MediaPosition(Duration::from_millis(1_500)));
        assert_eq!(
            playback_for(&state(PlayerStatus::Playing, 1_500)),
            MediaPlayback::Playing { progress }
        );
        assert_eq!(
            playback_for(&state(PlayerStatus::Paused, 1_500)),
            MediaPlayback::Paused { progress }
        );
        assert_eq!(
            playback_for(&state(PlayerStatus::Idle, 0)),
            MediaPlayback::Stopped
        );
    }
}
