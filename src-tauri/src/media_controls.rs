//! OS media controls: the Windows media overlay (SMTC) and macOS Now Playing, through
//! souvlaki. Player state flows in over a channel from the event sink; media keys flow back
//! as player commands. A worker thread owns the controls, so cover resizing and OS calls
//! never run on the audio engine thread. If the controls cannot start, Prism runs without.

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
use crate::player::{EventSink, PlayerEvent, PlayerState, PlayerStatus};
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
    pub duration: Duration,
}

impl OwnedMetadata {
    pub fn as_metadata(&self) -> MediaMetadata<'_> {
        MediaMetadata {
            title: Some(&self.title),
            album: Some(&self.album),
            artist: Some(&self.artist),
            cover_url: self.cover_url.as_deref(),
            duration: Some(self.duration),
        }
    }
}

/// Maps a library track (and its cached cover file, if any) to OS metadata.
pub fn to_metadata(track: &Track, cover_file: Option<&Path>) -> OwnedMetadata {
    OwnedMetadata {
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: track.album.clone(),
        cover_url: cover_file.map(|path| format!("file://{}", path.display())),
        duration: Duration::from_millis(track.duration_ms),
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
pub fn action_for(event: &MediaControlEvent, state: &PlayerState) -> Option<Action> {
    let seek_by = |direction: SeekDirection, ms: u64| {
        let target = match direction {
            SeekDirection::Forward => state.position_ms.saturating_add(ms),
            SeekDirection::Backward => state.position_ms.saturating_sub(ms),
        };
        Some(Action::Seek(target.min(state.duration_ms)))
    };
    match event {
        MediaControlEvent::Play => (state.status == PlayerStatus::Paused).then_some(Action::Toggle),
        MediaControlEvent::Pause | MediaControlEvent::Stop => {
            (state.status == PlayerStatus::Playing).then_some(Action::Toggle)
        }
        MediaControlEvent::Toggle => Some(Action::Toggle),
        MediaControlEvent::Next => Some(Action::Next),
        MediaControlEvent::Previous => Some(Action::Prev),
        MediaControlEvent::SetPosition(MediaPosition(position)) => Some(Action::Seek(
            (position.as_millis() as u64).min(state.duration_ms),
        )),
        MediaControlEvent::Seek(direction) => seek_by(*direction, SEEK_STEP_MS),
        MediaControlEvent::SeekBy(direction, amount) => {
            seek_by(*direction, amount.as_millis() as u64)
        }
        _ => None,
    }
}

fn playback_for(state: &PlayerState) -> MediaPlayback {
    let progress = Some(MediaPosition(Duration::from_millis(state.position_ms)));
    match state.status {
        PlayerStatus::Idle => MediaPlayback::Stopped,
        PlayerStatus::Loading | PlayerStatus::Paused => MediaPlayback::Paused { progress },
        PlayerStatus::Playing => MediaPlayback::Playing { progress },
    }
}

/// What was last sent to the OS.
#[derive(Default)]
struct Pushed {
    track_id: Option<String>,
    status: Option<PlayerStatus>,
    position_ms: u64,
    at: Option<Instant>,
}

type CoverFor = Box<dyn Fn(&Track) -> Option<PathBuf>>;

fn push(
    controls: &mut MediaControls,
    pushed: &mut Pushed,
    state: &PlayerState,
    cover_for: &CoverFor,
) {
    let track_id = state.track.as_ref().map(|track| track.id.as_str());
    let track_changed = track_id != pushed.track_id.as_deref();
    let status_changed = Some(state.status) != pushed.status;
    let position_due = match state.status {
        PlayerStatus::Playing => pushed.at.is_none_or(|at| at.elapsed() >= POSITION_INTERVAL),
        // A seek while paused must show up too.
        _ => state.position_ms != pushed.position_ms,
    };
    if !(track_changed || status_changed || position_due) {
        return;
    }
    if track_changed {
        if let Some(track) = &state.track {
            let cover = cover_for(track);
            let metadata = to_metadata(track, cover.as_deref());
            if let Err(err) = controls.set_metadata(metadata.as_metadata()) {
                eprintln!("updating the media metadata failed: {err:?}");
            }
        }
    }
    if let Err(err) = controls.set_playback(playback_for(state)) {
        eprintln!("updating the media playback state failed: {err:?}");
    }
    pushed.track_id = track_id.map(str::to_string);
    pushed.status = Some(state.status);
    pushed.position_ms = state.position_ms;
    pushed.at = Some(Instant::now());
}

/// The player-state side of the OS controls. [`sink`](MediaBridge::sink) goes into the
/// player's event fan-out; [`start`](MediaBridge::start) runs once the main window exists.
pub struct MediaBridge {
    states: Sender<PlayerState>,
    pending: Mutex<Option<Receiver<PlayerState>>>,
}

impl MediaBridge {
    pub fn new() -> Arc<MediaBridge> {
        let (states, receiver) = crossbeam_channel::unbounded();
        Arc::new(MediaBridge {
            states,
            pending: Mutex::new(Some(receiver)),
        })
    }

    /// Forwards state events to the worker; never blocks.
    pub fn sink(self: &Arc<Self>) -> EventSink {
        let bridge = Arc::clone(self);
        Arc::new(move |event| {
            if let PlayerEvent::State(state) = event {
                let _ = bridge.states.send(state);
            }
        })
    }

    /// Starts the worker. `hwnd` is the main window handle (Windows) or `None`. The worker
    /// keeps draining states even when the OS controls fail to initialise.
    pub fn start(&self, app: AppHandle, hwnd: Option<usize>) {
        let receiver = match self.pending.lock() {
            Ok(mut pending) => pending.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        let Some(receiver) = receiver else { return };
        let spawned = std::thread::Builder::new()
            .name("prism-media-controls".into())
            .spawn(move || {
                let run = catch_unwind(AssertUnwindSafe(|| run(receiver, app, hwnd)));
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
        let Some(state) = handle.try_state::<AppState>() else {
            return;
        };
        let Some(action) = action_for(&event, &state.player.state()) else {
            return;
        };
        let result = match action {
            Action::Toggle => state.player.toggle(),
            Action::Next => state.player.next(),
            Action::Prev => state.player.prev(),
            Action::Seek(ms) => state.player.seek(ms),
        };
        if let Err(err) = result {
            eprintln!("a media key could not be applied: {err}");
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

fn run(receiver: Receiver<PlayerState>, app: AppHandle, hwnd: Option<usize>) {
    let mut controls = create(&app, hwnd);
    let cover_app = app.clone();
    let cover_for: CoverFor = Box::new(move |track| {
        let id = track.cover_id.as_deref()?;
        let state = cover_app.try_state::<AppState>()?;
        let store = &state.library.covers;
        crate::protocol::cached_cover(store, store.dir().parent()?, id, COVER_SIZE)
    });
    let mut pushed = Pushed::default();
    while let Ok(mut state) = receiver.recv() {
        // Only the newest state matters.
        while let Ok(newer) = receiver.try_recv() {
            state = newer;
        }
        if let Some(controls) = controls.as_mut() {
            push(controls, &mut pushed, &state, &cover_for);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::model::Source;

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

    fn state(status: PlayerStatus, position_ms: u64) -> PlayerState {
        PlayerState {
            status,
            track: Some(track()),
            position_ms,
            duration_ms: 215_500,
            volume: 0.8,
            queue_index: 0,
            queue_length: 1,
        }
    }

    #[test]
    fn maps_track_to_metadata() {
        let cover = Path::new("cache").join("covers").join("600").join("c1.jpg");
        let metadata = to_metadata(&track(), Some(&cover));
        assert_eq!(metadata.title, "Song");
        assert_eq!(metadata.artist, "Artist");
        assert_eq!(metadata.album, "Album");
        assert_eq!(metadata.duration, Duration::from_millis(215_500));
        assert_eq!(
            metadata.cover_url,
            Some(format!("file://{}", cover.display()))
        );
        let borrowed = metadata.as_metadata();
        assert_eq!(borrowed.title, Some("Song"));
        assert_eq!(borrowed.duration, Some(Duration::from_millis(215_500)));
    }

    #[test]
    fn no_cover_means_no_cover_url() {
        assert_eq!(to_metadata(&track(), None).cover_url, None);
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
