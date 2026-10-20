//! The player service: one engine thread owns the [`Playback`] and the [`Queue`]; commands
//! reach it over a channel and it reports through an event sink.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{select, Receiver, Sender};
use serde::Serialize;

use super::queue::Queue;
use crate::audio::gain::replaygain_factor;
use crate::audio::{
    open_source_with, AudioSource, Input, OutputFactory, Playback, PumpStatus, StreamSpec,
};
use crate::library::model::Track;
use crate::settings::ReplayGain;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayerStatus {
    Idle,
    Loading,
    Playing,
    Paused,
}

/// Payload of `player://state` and the result of `player_state`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerState {
    pub status: PlayerStatus,
    pub track: Option<Track>,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub volume: f32,
    pub queue_index: u32,
    pub queue_length: u32,
}

/// Payload of `player://error`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerError {
    pub track_id: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    /// The state changed (or 250 ms passed while playing).
    State(PlayerState),
    /// A track became audible.
    TrackChanged(Track),
    /// A track could not be opened or decoded; the player moves on.
    Error(PlayerError),
}

/// Looks a track id up (in the library DB).
pub type Resolver = Arc<dyn Fn(&str) -> Option<Track> + Send + Sync>;
/// Receives everything the player reports.
pub type EventSink = Arc<dyn Fn(PlayerEvent) + Send + Sync>;
/// Opens a track for decoding at the given output spec. Runs on the opener thread.
pub type Opener =
    Arc<dyn Fn(&Track, StreamSpec) -> anyhow::Result<Box<dyn AudioSource>> + Send + Sync>;

/// Volume before the settings say otherwise.
pub const DEFAULT_VOLUME: f32 = 0.8;

/// How the engine starts; each value can be changed later through [`Player`].
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerOptions {
    pub volume: f32,
    /// `None` is the system default output.
    pub device: Option<String>,
    pub gapless: bool,
    pub replay_gain: ReplayGain,
}

impl Default for PlayerOptions {
    fn default() -> Self {
        PlayerOptions {
            volume: DEFAULT_VOLUME,
            device: None,
            gapless: true,
            replay_gain: ReplayGain::Off,
        }
    }
}
/// The next track is opened this long before the current one ends, for gapless playback.
pub const PRELOAD_MS: u64 = 5_000;
/// `prev` restarts the current track once it has played longer than this.
pub const RESTART_AFTER_MS: u64 = 3_000;
/// `player://state` is repeated this often while playing.
const STATE_INTERVAL: Duration = Duration::from_millis(250);
/// How long the engine waits for a command between pumps while audio is moving...
const BUSY_TICK: Duration = Duration::from_millis(10);
/// ...and while nothing needs pumping.
const IDLE_TICK: Duration = Duration::from_millis(250);

impl PlayerState {
    fn idle(volume: f32) -> PlayerState {
        PlayerState {
            status: PlayerStatus::Idle,
            track: None,
            position_ms: 0,
            duration_ms: 0,
            volume,
            queue_index: 0,
            queue_length: 0,
        }
    }
}

/// Opens local files (symphonia, falling back to FFmpeg) at the output's rate.
pub fn default_opener() -> Opener {
    Arc::new(|track: &Track, spec: StreamSpec| {
        let path = track
            .path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("this track has no file to play"))?;
        open_source_with(&Input::File(path.into()), Some(spec))
    })
}

enum Command {
    Play(Vec<String>, usize),
    Toggle,
    Next,
    Prev,
    Seek(u64),
    SetVolume(f32),
    SetDevice(Option<String>),
    SetGapless(bool),
    SetReplayGain(ReplayGain),
    Shutdown,
}

/// 0..=1; anything that is not a number becomes the default.
fn clamp_volume(volume: f32) -> f32 {
    if volume.is_finite() {
        volume.clamp(0.0, 1.0)
    } else {
        DEFAULT_VOLUME
    }
}

/// The linear ReplayGain factor for `track` under `mode`. Album mode uses the track values
/// when the album has none; a track without tags plays at 1.0.
pub fn track_gain(track: &Track, mode: ReplayGain) -> f32 {
    match mode {
        ReplayGain::Off => 1.0,
        ReplayGain::Album if track.replay_gain_album_db.is_some() => replaygain_factor(
            track.replay_gain_album_db,
            track
                .replay_gain_album_peak
                .or(track.replay_gain_track_peak),
        ),
        ReplayGain::Track | ReplayGain::Album => {
            replaygain_factor(track.replay_gain_track_db, track.replay_gain_track_peak)
        }
    }
}

/// Handle to the engine thread. Every method only sends a command; none of them blocks on
/// audio work. Dropping it stops the engine.
pub struct Player {
    commands: Sender<Command>,
    state: Arc<Mutex<PlayerState>>,
    thread: Option<JoinHandle<()>>,
}

impl Player {
    pub fn spawn(factory: Arc<dyn OutputFactory>, resolver: Resolver, sink: EventSink) -> Player {
        Player::spawn_with(factory, resolver, sink, default_opener())
    }

    pub fn spawn_with(
        factory: Arc<dyn OutputFactory>,
        resolver: Resolver,
        sink: EventSink,
        opener: Opener,
    ) -> Player {
        Player::spawn_with_options(factory, resolver, sink, opener, PlayerOptions::default())
    }

    pub fn spawn_with_options(
        factory: Arc<dyn OutputFactory>,
        resolver: Resolver,
        sink: EventSink,
        opener: Opener,
        options: PlayerOptions,
    ) -> Player {
        let options = PlayerOptions {
            volume: clamp_volume(options.volume),
            ..options
        };
        let (commands, command_rx) = crossbeam_channel::unbounded();
        let state = Arc::new(Mutex::new(PlayerState::idle(options.volume)));
        let shared = Arc::clone(&state);
        let thread = std::thread::Builder::new()
            .name("prism-player".into())
            .spawn(move || {
                let run = catch_unwind(AssertUnwindSafe(|| {
                    match Engine::new(factory, resolver, sink, opener, shared, options) {
                        Ok(mut engine) => engine.run(command_rx),
                        Err(err) => eprintln!("the audio engine could not start: {err:#}"),
                    }
                }));
                if run.is_err() {
                    eprintln!("the audio engine stopped after a panic");
                }
            });
        let thread = thread
            .map_err(|err| eprintln!("starting the audio engine failed: {err}"))
            .ok();
        Player {
            commands,
            state,
            thread,
        }
    }

    fn send(&self, command: Command) -> Result<(), String> {
        self.commands
            .send(command)
            .map_err(|_| "The audio engine is not running.".to_string())
    }

    /// Replaces the queue with `ids` and starts playing at `start`.
    pub fn play(&self, ids: Vec<String>, start: usize) -> Result<(), String> {
        self.send(Command::Play(ids, start))
    }

    pub fn toggle(&self) -> Result<(), String> {
        self.send(Command::Toggle)
    }

    pub fn next(&self) -> Result<(), String> {
        self.send(Command::Next)
    }

    /// Restarts the track after [`RESTART_AFTER_MS`], otherwise goes to the previous one.
    pub fn prev(&self) -> Result<(), String> {
        self.send(Command::Prev)
    }

    pub fn seek(&self, ms: u64) -> Result<(), String> {
        self.send(Command::Seek(ms))
    }

    /// `volume` is clamped to 0..=1.
    pub fn set_volume(&self, volume: f32) -> Result<(), String> {
        if !volume.is_finite() {
            return Err("The volume must be a number between 0 and 1.".to_string());
        }
        self.send(Command::SetVolume(volume))
    }

    /// Moves playback to another output (`None` = the system default), keeping the
    /// position. A device that is gone falls back to the default.
    pub fn set_device(&self, device: Option<String>) -> Result<(), String> {
        self.send(Command::SetDevice(device))
    }

    /// Off: the next track is opened only once the current one has ended.
    pub fn set_gapless(&self, gapless: bool) -> Result<(), String> {
        self.send(Command::SetGapless(gapless))
    }

    /// Applies at once to the playing track and the pre-opened next one.
    pub fn set_replay_gain(&self, mode: ReplayGain) -> Result<(), String> {
        self.send(Command::SetReplayGain(mode))
    }

    /// The latest state, as the engine published it.
    pub fn state(&self) -> PlayerState {
        match self.state.lock() {
            Ok(state) => state.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// The track to play now.
    Current,
    /// The track after it, opened early for a gapless hand-over.
    Next,
}

/// Open the queue entry `id`. The opener thread resolves it (the resolver takes the
/// library DB lock, which a scan can hold for a while) and opens the file.
struct OpenJob {
    generation: u64,
    slot: Slot,
    index: usize,
    id: String,
    spec: StreamSpec,
}

struct Opened {
    generation: u64,
    slot: Slot,
    index: usize,
    id: String,
    result: Result<(Track, Box<dyn AudioSource>), String>,
}

/// Looks `id` up and checks that it can be played.
fn resolve(resolver: &Resolver, id: &str) -> Result<Track, String> {
    match resolver(id) {
        Some(track) if track.path.is_some() => Ok(track),
        Some(_) => Err("This track has no file to play.".to_string()),
        None => Err("This track is no longer in the library.".to_string()),
    }
}

/// Resolves and opens tracks off the engine thread: the resolver can wait on the library DB
/// and an open decodes the first packet (FFmpeg can take seconds). The engine thread never
/// touches either. Jobs from an older generation are skipped.
fn spawn_opener(
    resolver: Resolver,
    opener: Opener,
    generation: Arc<AtomicU64>,
    results: Sender<Opened>,
) -> Sender<OpenJob> {
    let (jobs, job_rx) = crossbeam_channel::unbounded::<OpenJob>();
    let spawned = std::thread::Builder::new()
        .name("prism-opener".into())
        .spawn(move || {
            for job in job_rx {
                if job.generation != generation.load(Ordering::SeqCst) {
                    continue;
                }
                let result = catch_unwind(AssertUnwindSafe(|| {
                    let track = resolve(&resolver, &job.id)?;
                    let source = opener(&track, job.spec).map_err(|err| format!("{err:#}"))?;
                    Ok((track, source))
                }))
                .unwrap_or_else(|_| Err("opening the file crashed".to_string()));
                let opened = Opened {
                    generation: job.generation,
                    slot: job.slot,
                    index: job.index,
                    id: job.id,
                    result,
                };
                if results.send(opened).is_err() {
                    break;
                }
            }
        });
    if let Err(err) = spawned {
        // The job receiver is gone with the closure, so every open reports an error.
        eprintln!("starting the file opener failed: {err}");
    }
    jobs
}

/// The gapless pre-open of the track after the current one.
enum Preload {
    None,
    Requested(usize),
    Ready(Box<Track>),
    /// (index, track id, message): reported when the queue reaches it.
    Failed(usize, String, String),
}

/// Lives on the engine thread and owns everything that is not `Send`.
struct Engine {
    playback: Playback,
    queue: Queue,
    sink: EventSink,
    shared: Arc<Mutex<PlayerState>>,
    jobs: Sender<OpenJob>,
    opened: Receiver<Opened>,
    generation: Arc<AtomicU64>,
    status: PlayerStatus,
    volume: f32,
    gapless: bool,
    replay_gain: ReplayGain,
    /// The user paused while the track was loading: start it paused.
    start_paused: bool,
    /// The track being decoded. Between a gapless hand-over and the moment it is heard it
    /// is already the next track while `shown` is still the previous one.
    current: Option<Track>,
    /// `current` still has a source in the playback.
    decoding: bool,
    shown: Option<Track>,
    shown_index: usize,
    /// An opened source waiting for the previous track's tail to play out.
    ready: Option<Box<dyn AudioSource>>,
    preload: Preload,
    /// A gapless hand-over and the instant it becomes audible.
    handover: Option<(Instant, Track)>,
    /// The queue ran out; idle once the output has played everything.
    ending: bool,
    dirty: bool,
    last_emit: Instant,
}

impl Engine {
    fn new(
        factory: Arc<dyn OutputFactory>,
        resolver: Resolver,
        sink: EventSink,
        opener: Opener,
        shared: Arc<Mutex<PlayerState>>,
        options: PlayerOptions,
    ) -> anyhow::Result<Engine> {
        let mut playback = Playback::new(factory, options.device.clone())?;
        playback.set_volume(options.volume);
        if let Some(err) = playback.output_error() {
            eprintln!("no audio output yet: {err}");
        }
        let generation = Arc::new(AtomicU64::new(0));
        let (results, opened) = crossbeam_channel::unbounded();
        let jobs = spawn_opener(resolver, opener, Arc::clone(&generation), results);
        Ok(Engine {
            playback,
            queue: Queue::new(),
            sink,
            shared,
            jobs,
            opened,
            generation,
            status: PlayerStatus::Idle,
            volume: options.volume,
            gapless: options.gapless,
            replay_gain: options.replay_gain,
            start_paused: false,
            current: None,
            decoding: false,
            shown: None,
            shown_index: 0,
            ready: None,
            preload: Preload::None,
            handover: None,
            ending: false,
            dirty: true,
            last_emit: Instant::now(),
        })
    }

    fn run(&mut self, commands: Receiver<Command>) {
        let opened = self.opened.clone();
        self.publish();
        loop {
            let timeout = if self.busy() { BUSY_TICK } else { IDLE_TICK };
            select! {
                recv(commands) -> command => match command {
                    Ok(Command::Shutdown) | Err(_) => break,
                    Ok(command) => self.handle(command),
                },
                recv(opened) -> result => {
                    if let Ok(result) = result {
                        self.on_opened(result);
                    }
                },
                default(timeout) => {}
            }
            self.tick();
            self.publish();
        }
        self.playback.stop();
    }

    fn busy(&self) -> bool {
        matches!(self.status, PlayerStatus::Playing | PlayerStatus::Loading)
            || self.handover.is_some()
            || self.ready.is_some()
            || self.ending
    }

    fn handle(&mut self, command: Command) {
        // Nothing to skip while idle (play/toggle start the queue); an ignored command
        // changes nothing and emits nothing.
        if matches!(command, Command::Next | Command::Prev) && self.status == PlayerStatus::Idle {
            return;
        }
        self.dirty = true;
        match command {
            Command::Play(ids, start) => {
                self.queue.set(ids, start);
                self.start_current(true);
            }
            Command::Toggle => self.toggle(),
            Command::Next => {
                self.settle_handover();
                if self.queue.advance() {
                    self.start_current(true);
                } else {
                    self.go_idle();
                }
            }
            Command::Prev => self.prev(),
            Command::Seek(ms) => self.seek(ms),
            Command::SetVolume(volume) => {
                self.volume = volume.clamp(0.0, 1.0);
                self.playback.set_volume(self.volume);
            }
            Command::SetDevice(device) => {
                if let Err(err) = self.playback.set_device(device) {
                    self.fail_current(err);
                }
            }
            Command::SetGapless(gapless) => {
                self.gapless = gapless;
                if !gapless {
                    // A track opened early is dropped; it opens again once this one ends.
                    self.preload = Preload::None;
                    self.playback.queue_next(None);
                }
            }
            Command::SetReplayGain(mode) => {
                self.replay_gain = mode;
                if let Some(track) = &self.current {
                    self.playback.set_gain(track_gain(track, mode));
                }
                if let Preload::Ready(track) = &self.preload {
                    self.playback.set_next_gain(track_gain(track, mode));
                }
            }
            Command::Shutdown => {}
        }
    }

    fn toggle(&mut self) {
        match self.status {
            PlayerStatus::Playing => {
                self.playback.set_paused(true);
                self.status = PlayerStatus::Paused;
            }
            PlayerStatus::Paused => {
                self.playback.set_paused(false);
                self.status = PlayerStatus::Playing;
            }
            PlayerStatus::Loading => self.start_paused = !self.start_paused,
            PlayerStatus::Idle => {
                if !self.queue.is_empty() {
                    self.start_current(true);
                }
            }
        }
    }

    /// True while the current track can be seeked.
    fn seekable(&self) -> bool {
        self.decoding && matches!(self.status, PlayerStatus::Playing | PlayerStatus::Paused)
    }

    fn prev(&mut self) {
        self.settle_handover();
        if self.seekable() && self.position_ms() > RESTART_AFTER_MS {
            self.seek(0);
        } else if self.queue.back() {
            self.start_current(true);
        } else if self.seekable() {
            self.seek(0);
        }
    }

    fn seek(&mut self, ms: u64) {
        if !self.seekable() {
            return;
        }
        self.settle_handover();
        let duration = self.current.as_ref().map_or(0, |track| track.duration_ms);
        let ms = if duration > 0 { ms.min(duration) } else { ms };
        if let Err(err) = self.playback.seek(ms) {
            self.fail_current(err);
        }
    }

    /// Starts loading the queue's current track (resolved and opened on the opener thread;
    /// a failure is reported and skipped when the result comes back). `cut` stops what is
    /// playing now; otherwise the previous track's tail plays out first.
    fn start_current(&mut self, cut: bool) {
        self.settle_handover();
        if cut {
            self.playback.stop();
            self.decoding = false;
        }
        self.ready = None;
        self.preload = Preload::None;
        self.ending = false;
        self.start_paused = false;
        self.dirty = true;
        let Some(id) = self.queue.current().map(str::to_string) else {
            self.finish(cut);
            return;
        };
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        // The track is known once the opener has resolved it. Until then the previous
        // track stays shown (its tail may still be playing); only a start from idle shows
        // nothing.
        self.current = None;
        self.status = PlayerStatus::Loading;
        let job = OpenJob {
            generation,
            slot: Slot::Current,
            index: self.queue.index(),
            id: id.clone(),
            spec: self.playback.output_spec(),
        };
        if self.jobs.send(job).is_err() {
            self.error(&id, "the file opener is not running");
            // Every other track would fail the same way.
            self.finish(cut);
        }
    }

    /// Moves past a track that failed (or ended): the next one, or the end of the queue.
    fn skip(&mut self, cut: bool) {
        if self.queue.advance() {
            self.start_current(cut);
        } else {
            self.finish(cut);
        }
    }

    /// The queue is done: idle now (`cut`), or once the output has played everything.
    fn finish(&mut self, cut: bool) {
        if cut {
            self.go_idle();
        } else {
            self.current = None;
            self.ending = true;
        }
    }

    fn go_idle(&mut self) {
        self.playback.stop();
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.status = PlayerStatus::Idle;
        self.current = None;
        self.decoding = false;
        self.shown = None;
        self.shown_index = 0;
        self.ready = None;
        self.preload = Preload::None;
        self.handover = None;
        self.ending = false;
        self.start_paused = false;
        self.queue.rewind();
        self.dirty = true;
    }

    fn on_opened(&mut self, opened: Opened) {
        if opened.generation != self.generation.load(Ordering::SeqCst) {
            return;
        }
        match opened.slot {
            Slot::Current => {
                if opened.index != self.queue.index() || self.status != PlayerStatus::Loading {
                    return;
                }
                match opened.result {
                    Ok((track, source)) => {
                        self.current = Some(track.clone());
                        self.shown = Some(track);
                        self.shown_index = self.queue.index();
                        self.dirty = true;
                        self.ready = Some(source);
                    }
                    Err(message) => {
                        self.error(&opened.id, &message);
                        self.skip(false);
                    }
                }
            }
            Slot::Next => {
                if !matches!(self.preload, Preload::Requested(index) if index == opened.index) {
                    return;
                }
                self.preload = match opened.result {
                    Ok((track, source)) => {
                        self.playback.queue_next(Some(source));
                        self.playback
                            .set_next_gain(track_gain(&track, self.replay_gain));
                        Preload::Ready(Box::new(track))
                    }
                    Err(message) => Preload::Failed(opened.index, opened.id, message),
                };
            }
        }
    }

    fn tick(&mut self) {
        // While paused nothing is heard, so a pending hand-over waits.
        if self.status != PlayerStatus::Paused {
            if let Some((at, _)) = &self.handover {
                if Instant::now() >= *at {
                    self.settle_handover();
                }
            }
        }
        if self.ready.is_some() && self.playback.is_drained() {
            self.start_ready();
        }
        self.maybe_preload();
        if matches!(self.status, PlayerStatus::Playing | PlayerStatus::Loading) {
            match self.playback.pump() {
                Ok(PumpStatus::Playing | PumpStatus::NoDevice) => {}
                Ok(PumpStatus::Advanced) => self.on_advanced(),
                Ok(PumpStatus::Finished) => {
                    if self.decoding {
                        self.on_finished();
                    }
                }
                Err(err) => self.fail_current(err),
            }
        }
        if self.ending && self.playback.is_drained() {
            self.go_idle();
        }
    }

    /// The loaded track starts playing (the previous one has played out).
    fn start_ready(&mut self) {
        let Some(source) = self.ready.take() else {
            return;
        };
        match self.playback.set_source(source) {
            Ok(()) => {
                if let Some(track) = &self.current {
                    self.playback.set_gain(track_gain(track, self.replay_gain));
                }
                self.decoding = true;
                let paused = std::mem::take(&mut self.start_paused);
                self.playback.set_paused(paused);
                self.status = if paused {
                    PlayerStatus::Paused
                } else {
                    PlayerStatus::Playing
                };
                if let Some(track) = self.current.clone() {
                    self.show(track);
                }
            }
            Err(err) => {
                let id = self.current_id();
                self.error(&id, &format!("{err:#}"));
                self.skip(false);
            }
        }
    }

    /// Opens the next track once the current one is within [`PRELOAD_MS`] of its end.
    fn maybe_preload(&mut self) {
        if !self.gapless || !matches!(self.preload, Preload::None) || !self.seekable() {
            return;
        }
        let Some(track) = &self.current else {
            return;
        };
        let Some(index) = self.queue.next_index() else {
            return;
        };
        // Durations come from the library (FFmpeg sources do not know theirs).
        if track.duration_ms == 0 {
            return;
        }
        let remaining = track
            .duration_ms
            .saturating_sub(self.playback.position_ms());
        if remaining > PRELOAD_MS {
            return;
        }
        let Some(id) = self.queue.get(index).map(str::to_string) else {
            return;
        };
        let job = OpenJob {
            generation: self.generation.load(Ordering::SeqCst),
            slot: Slot::Next,
            index,
            id: id.clone(),
            spec: self.playback.output_spec(),
        };
        self.preload = match self.jobs.send(job) {
            Ok(()) => Preload::Requested(index),
            Err(_) => Preload::Failed(index, id, "the file opener is not running".into()),
        };
    }

    /// The pre-opened next track took over; the UI hears about it once it is audible.
    fn on_advanced(&mut self) {
        let Preload::Ready(track) = std::mem::replace(&mut self.preload, Preload::None) else {
            return;
        };
        self.settle_handover();
        self.queue.advance();
        self.current = Some((*track).clone());
        self.decoding = true;
        let audible_at = Instant::now() + Duration::from_millis(self.playback.queued_ms());
        self.handover = Some((audible_at, *track));
    }

    /// The current track ended with nothing queued behind it.
    fn on_finished(&mut self) {
        self.decoding = false;
        match std::mem::replace(&mut self.preload, Preload::None) {
            Preload::Failed(index, id, message) if Some(index) == self.queue.next_index() => {
                self.queue.advance();
                self.error(&id, &message);
                self.skip(false);
            }
            _ => self.skip(false),
        }
    }

    /// A decode or seek error: report it and move on.
    fn fail_current(&mut self, err: anyhow::Error) {
        let id = self.current_id();
        // A hand-over to the failing track was never heard.
        self.handover = None;
        self.error(&id, &format!("{err:#}"));
        self.playback.stop();
        self.decoding = false;
        self.skip(true);
    }

    fn current_id(&self) -> String {
        self.current
            .as_ref()
            .map(|track| track.id.clone())
            .unwrap_or_default()
    }

    fn settle_handover(&mut self) {
        if let Some((_, track)) = self.handover.take() {
            self.show(track);
        }
    }

    /// `track` is now audible.
    fn show(&mut self, track: Track) {
        self.shown = Some(track.clone());
        self.shown_index = self.queue.index();
        self.dirty = true;
        (self.sink)(PlayerEvent::TrackChanged(track));
    }

    fn error(&mut self, track_id: &str, message: &str) {
        self.dirty = true;
        (self.sink)(PlayerEvent::Error(PlayerError {
            track_id: track_id.to_string(),
            message: message.to_string(),
        }));
    }

    fn position_ms(&self) -> u64 {
        let duration = self.shown.as_ref().map_or(0, |track| track.duration_ms);
        if self.handover.is_some() {
            return duration;
        }
        match self.status {
            PlayerStatus::Playing | PlayerStatus::Paused if self.decoding => {
                let position = self.playback.position_ms();
                if duration > 0 {
                    position.min(duration)
                } else {
                    position
                }
            }
            PlayerStatus::Playing | PlayerStatus::Paused => duration,
            PlayerStatus::Idle | PlayerStatus::Loading => 0,
        }
    }

    fn snapshot(&self) -> PlayerState {
        PlayerState {
            status: self.status,
            track: self.shown.clone(),
            position_ms: self.position_ms(),
            duration_ms: self.shown.as_ref().map_or(0, |track| track.duration_ms),
            volume: self.volume,
            queue_index: self.shown_index as u32,
            queue_length: self.queue.len() as u32,
        }
    }

    /// When the state changed, or every [`STATE_INTERVAL`] while playing: updates the
    /// shared state (what `player_state` returns) and emits it.
    fn publish(&mut self) {
        let periodic =
            self.status == PlayerStatus::Playing && self.last_emit.elapsed() >= STATE_INTERVAL;
        if !self.dirty && !periodic {
            return;
        }
        self.dirty = false;
        self.last_emit = Instant::now();
        let state = self.snapshot();
        match self.shared.lock() {
            Ok(mut shared) => *shared = state.clone(),
            Err(poisoned) => *poisoned.into_inner() = state.clone(),
        }
        (self.sink)(PlayerEvent::State(state));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use crossbeam_channel::{Receiver, Sender};

    use super::*;
    use crate::audio::{open_source, Input, OutputEvent, OutputSink};
    use crate::library::tags::{self, fixtures};
    use crate::player::dropped::tests::drop_files;
    use crate::player::dropped::{with_dropped, DroppedFiles};

    const STEREO_44K: StreamSpec = StreamSpec {
        sample_rate: 44_100,
        channels: 2,
    };

    /// An output that plays `speed` times faster than real time and records what it played.
    /// Samples are played the moment they are taken, so nothing is ever queued.
    ///
    /// Like the real output (a ring the device drains, silence on under-run) it does not bank
    /// idle time: room grows with time passed (sped up) but never beyond one ring of
    /// [`RING_MS`], so a late first track cannot be swallowed whole in one push.
    struct InstantSink {
        spec: StreamSpec,
        speed: f64,
        /// When `room` was last brought up to date.
        last: Instant,
        /// Frames the device could take right now.
        room: f64,
        played: Arc<Mutex<Vec<f32>>>,
        events: Receiver<OutputEvent>,
        _keep: Sender<OutputEvent>,
    }

    impl OutputSink for InstantSink {
        fn spec(&self) -> StreamSpec {
            self.spec
        }
        fn push(&mut self, samples: &[f32]) -> usize {
            let channels = self.spec.channels as usize;
            let rate = self.spec.sample_rate as f64;
            let now = Instant::now();
            let grown = now.duration_since(self.last).as_secs_f64() * self.speed * rate;
            self.last = now;
            self.room = (self.room + grown).min(rate * RING_MS as f64 / 1000.0);
            let frames = (self.room as usize).min(samples.len() / channels);
            self.room -= frames as f64;
            let taken = frames * channels;
            self.played
                .lock()
                .unwrap()
                .extend_from_slice(&samples[..taken]);
            taken
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

    struct InstantFactory {
        speed: f64,
        played: Arc<Mutex<Vec<f32>>>,
    }

    impl OutputFactory for InstantFactory {
        fn open(&self, _device: Option<String>) -> anyhow::Result<Box<dyn OutputSink>> {
            let (tx, rx) = crossbeam_channel::unbounded();
            Ok(Box::new(InstantSink {
                spec: STEREO_44K,
                speed: self.speed,
                last: Instant::now(),
                room: 0.0,
                played: Arc::clone(&self.played),
                events: rx,
                _keep: tx,
            }))
        }
    }

    /// Ten seconds of silence at the output spec.
    struct Silence {
        frames_left: u64,
    }

    const SILENCE_MS: u64 = 10_000;

    impl AudioSource for Silence {
        fn spec(&self) -> StreamSpec {
            STEREO_44K
        }
        fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize> {
            let frames = self.frames_left.min(1024) as usize;
            out.extend(std::iter::repeat_n(0.0, frames * 2));
            self.frames_left -= frames as u64;
            Ok(frames)
        }
        fn seek(&mut self, ms: u64) -> anyhow::Result<()> {
            self.frames_left = (SILENCE_MS.saturating_sub(ms)) * 44_100 / 1000;
            Ok(())
        }
        fn duration_ms(&self) -> Option<u64> {
            Some(SILENCE_MS)
        }
    }

    /// A track for `file` in the fixtures, under the id `id`.
    fn fixture_track(id: &str, file: &str) -> Track {
        let path = fixtures().join(file);
        let mut track = match tags::read(&path) {
            Ok(meta) => meta.track,
            Err(_) => tags::read(&fixtures().join("tagged.flac")).unwrap().track,
        };
        track.id = id.to_string();
        track.path = Some(path.to_string_lossy().into_owned());
        track
    }

    struct Harness {
        player: Player,
        events: Receiver<PlayerEvent>,
        played: Arc<Mutex<Vec<f32>>>,
    }

    /// The real output's ring (see `audio::output`): the most it holds ahead of the device.
    const RING_MS: u64 = 200;

    /// Plays 20 times faster than real time: a 1 s fixture takes 50 ms.
    const FAST: f64 = 20.0;

    fn harness(tracks: Vec<Track>, speed: f64, opener: Option<Opener>) -> Harness {
        let resolver: Resolver =
            Arc::new(move |id: &str| tracks.iter().find(|track| track.id == id).cloned());
        harness_with(resolver, speed, opener)
    }

    fn harness_with(resolver: Resolver, speed: f64, opener: Option<Opener>) -> Harness {
        harness_full(resolver, speed, opener, PlayerOptions::default())
    }

    fn harness_options(tracks: Vec<Track>, options: PlayerOptions) -> Harness {
        let resolver: Resolver =
            Arc::new(move |id: &str| tracks.iter().find(|track| track.id == id).cloned());
        harness_full(resolver, FAST, None, options)
    }

    fn harness_full(
        resolver: Resolver,
        speed: f64,
        opener: Option<Opener>,
        options: PlayerOptions,
    ) -> Harness {
        let played = Arc::new(Mutex::new(Vec::new()));
        let factory = Arc::new(InstantFactory {
            speed,
            played: Arc::clone(&played),
        });
        let (tx, events) = crossbeam_channel::unbounded();
        let sink: EventSink = Arc::new(move |event| {
            let _ = tx.send(event);
        });
        let opener = opener.unwrap_or_else(default_opener);
        let player = Player::spawn_with_options(factory, resolver, sink, opener, options);
        Harness {
            player,
            events,
            played,
        }
    }

    /// Collects events until `done` says so; panics after `timeout`.
    fn collect_until(
        events: &Receiver<PlayerEvent>,
        timeout: Duration,
        done: impl Fn(&[PlayerEvent]) -> bool,
    ) -> Vec<PlayerEvent> {
        let deadline = Instant::now() + timeout;
        let mut seen = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match events.recv_timeout(left) {
                Ok(event) => {
                    seen.push(event);
                    if done(&seen) {
                        return seen;
                    }
                }
                Err(_) => panic!("timed out; events so far: {seen:#?}"),
            }
        }
    }

    fn changes(events: &[PlayerEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                PlayerEvent::TrackChanged(track) => Some(track.id.clone()),
                _ => None,
            })
            .collect()
    }

    fn errors(events: &[PlayerEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                PlayerEvent::Error(error) => Some(error.track_id.clone()),
                _ => None,
            })
            .collect()
    }

    fn is_state(event: &PlayerEvent, status: PlayerStatus) -> bool {
        matches!(event, PlayerEvent::State(state) if state.status == status)
    }

    /// Idle again after the queue was started (a `Loading` state was seen).
    fn back_to_idle(events: &[PlayerEvent]) -> bool {
        let Some(started) = events
            .iter()
            .position(|event| is_state(event, PlayerStatus::Loading))
        else {
            return false;
        };
        events[started..]
            .iter()
            .any(|event| is_state(event, PlayerStatus::Idle))
    }

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn plays_queue_in_order() {
        let h = harness(
            vec![
                fixture_track("mp3", "tagged.mp3"),
                fixture_track("flac", "tagged.flac"),
            ],
            FAST,
            None,
        );
        h.player.play(ids(&["mp3", "flac"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), back_to_idle);
        assert_eq!(changes(&events), ids(&["mp3", "flac"]));
        assert!(errors(&events).is_empty());
        let state = h.player.state();
        assert_eq!(state.status, PlayerStatus::Idle);
        assert_eq!(state.queue_length, 2);
    }

    #[test]
    fn dropped_file_plays_without_indexing() {
        let dropped = Arc::new(DroppedFiles::new(None));
        let file = fixtures().join("tagged.mp3").to_string_lossy().into_owned();
        let ids = drop_files(&dropped, &[file]);
        // The library knows nothing: only the dropped-files map can resolve the id.
        let h = harness_with(
            with_dropped(dropped, Arc::new(|_id: &str| None)),
            FAST,
            None,
        );
        h.player.play(ids.clone(), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), back_to_idle);
        assert_eq!(changes(&events), ids);
        assert!(errors(&events).is_empty());
        assert!(
            !h.played.lock().unwrap().is_empty(),
            "samples reached the output"
        );
    }

    #[test]
    fn skips_corrupt_and_continues() {
        let h = harness(
            vec![
                fixture_track("bad", "corrupt.flac"),
                fixture_track("mp3", "tagged.mp3"),
            ],
            FAST,
            None,
        );
        h.player.play(ids(&["bad", "mp3"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), back_to_idle);
        assert_eq!(errors(&events), ids(&["bad"]));
        assert_eq!(changes(&events), ids(&["mp3"]));
        let error_at = events
            .iter()
            .position(|e| matches!(e, PlayerEvent::Error(_)))
            .unwrap();
        let change_at = events
            .iter()
            .position(|e| matches!(e, PlayerEvent::TrackChanged(_)))
            .unwrap();
        assert!(
            error_at < change_at,
            "the error comes first, then mp3 plays"
        );
    }

    #[test]
    fn all_failing_queue_goes_idle() {
        let started = Instant::now();
        let h = harness(
            vec![
                fixture_track("bad1", "corrupt.flac"),
                fixture_track("bad2", "corrupt.flac"),
            ],
            FAST,
            None,
        );
        h.player.play(ids(&["bad1", "bad2"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(5), |seen| {
            errors(seen).len() == 2 && back_to_idle(seen)
        });
        assert_eq!(errors(&events), ids(&["bad1", "bad2"]));
        assert!(changes(&events).is_empty());
        assert_eq!(h.player.state().status, PlayerStatus::Idle);
        // Nothing keeps trying: no further events arrive.
        assert!(h.events.recv_timeout(Duration::from_millis(300)).is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// Waits until `player.state()` satisfies `ok`.
    #[track_caller]
    fn wait_state(player: &Player, ok: impl Fn(&PlayerState) -> bool) -> PlayerState {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let state = player.state();
            if ok(&state) {
                return state;
            }
            assert!(Instant::now() < deadline, "state never matched: {state:#?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn prev_restarts_after_3s() {
        let mut first = fixture_track("long1", "tagged.flac");
        first.duration_ms = SILENCE_MS;
        let mut second = fixture_track("long2", "tagged.flac");
        second.duration_ms = SILENCE_MS;
        let opener: Opener = Arc::new(|_track: &Track, _spec: StreamSpec| {
            Ok(Box::new(Silence {
                frames_left: SILENCE_MS * 44_100 / 1000,
            }) as Box<dyn AudioSource>)
        });
        let h = harness(vec![first, second], FAST, Some(opener));
        h.player.play(ids(&["long1", "long2"]), 1).unwrap();
        wait_state(&h.player, |s| s.status == PlayerStatus::Playing);
        h.player.toggle().unwrap();
        wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
        h.player.seek(4_000).unwrap();
        wait_state(&h.player, |s| s.position_ms == 4_000);

        h.player.prev().unwrap();
        let state = wait_state(&h.player, |s| s.position_ms < 4_000);
        assert_eq!(state.queue_index, 1, "the same track restarts");
        assert_eq!(state.position_ms, 0);
        assert_eq!(state.track.unwrap().id, "long2");

        // Within the first 3 s, prev goes to the previous track.
        h.player.prev().unwrap();
        let state = wait_state(&h.player, |s| s.queue_index == 0 && s.track.is_some());
        assert_eq!(state.track.unwrap().id, "long1");
    }

    #[test]
    fn manual_next_keeps_a_track_shown() {
        let tracks: Vec<Track> = ["long1", "long2"]
            .iter()
            .map(|id| {
                let mut track = fixture_track(id, "tagged.flac");
                track.duration_ms = SILENCE_MS;
                track
            })
            .collect();
        let opener: Opener = Arc::new(|_track: &Track, _spec: StreamSpec| {
            Ok(Box::new(Silence {
                frames_left: SILENCE_MS * 44_100 / 1000,
            }) as Box<dyn AudioSource>)
        });
        let h = harness(tracks, FAST, Some(opener));
        h.player.play(ids(&["long1", "long2"]), 0).unwrap();
        collect_until(&h.events, Duration::from_secs(5), |seen| {
            changes(seen) == ids(&["long1"])
        });
        h.player.next().unwrap();
        let events = collect_until(&h.events, Duration::from_secs(5), |seen| {
            seen.iter().any(|e| {
                matches!(e, PlayerEvent::State(s)
                    if s.status == PlayerStatus::Playing
                        && s.track.as_ref().is_some_and(|t| t.id == "long2"))
            })
        });
        let states: Vec<&PlayerState> = events
            .iter()
            .filter_map(|e| match e {
                PlayerEvent::State(state) => Some(state),
                _ => None,
            })
            .collect();
        assert!(
            states.iter().any(|s| s.status == PlayerStatus::Loading),
            "the skip went through loading"
        );
        for state in &states {
            let track = state
                .track
                .as_ref()
                .expect("a track is shown while skipping");
            // The index always belongs to the track that is shown.
            let expected = if track.id == "long1" { 0 } else { 1 };
            assert_eq!(state.queue_index, expected, "{state:#?}");
        }
    }

    #[test]
    fn next_and_prev_do_nothing_while_idle() {
        let h = harness(
            vec![
                fixture_track("a", "gap_a.wav"),
                fixture_track("b", "gap_b.wav"),
            ],
            FAST,
            None,
        );
        h.player.play(ids(&["a", "b"]), 0).unwrap();
        collect_until(&h.events, Duration::from_secs(10), back_to_idle);
        h.player.next().unwrap();
        h.player.prev().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let later: Vec<PlayerEvent> = h.events.try_iter().collect();
        assert!(
            later.is_empty(),
            "ignored commands emit nothing: {later:#?}"
        );
        assert_eq!(h.player.state().status, PlayerStatus::Idle);
    }

    #[test]
    fn slow_resolver_does_not_starve_the_output() {
        let mut long = fixture_track("long", "tagged.flac");
        long.duration_ms = SILENCE_MS;
        let mut slow = fixture_track("slow", "tagged.flac");
        slow.duration_ms = SILENCE_MS;
        let tracks = [long, slow];
        let (started_tx, started_rx) = crossbeam_channel::unbounded();
        // Looking up "slow" holds the (library DB) lock for 500 ms, as a scan chunk would.
        let resolver: Resolver = Arc::new(move |id: &str| {
            if id == "slow" {
                let _ = started_tx.send(());
                std::thread::sleep(Duration::from_millis(500));
            }
            tracks.iter().find(|track| track.id == id).cloned()
        });
        let opener: Opener = Arc::new(|_track: &Track, _spec: StreamSpec| {
            Ok(Box::new(Silence {
                frames_left: SILENCE_MS * 44_100 / 1000,
            }) as Box<dyn AudioSource>)
        });
        let played = Arc::new(Mutex::new(Vec::new()));
        // 5x: the pre-open of "slow" starts 1 s in, with 1 s of "long" left to play.
        let factory = Arc::new(InstantFactory {
            speed: 5.0,
            played: Arc::clone(&played),
        });
        let player = Player::spawn_with(factory, resolver, Arc::new(|_| {}), opener);
        player.play(ids(&["long", "slow"]), 0).unwrap();

        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the next track is resolved");
        let before = played.lock().unwrap().len();
        std::thread::sleep(Duration::from_millis(300));
        let after = played.lock().unwrap().len();
        // 300 ms at 5x is 1.5 s of audio; require at least a third of it.
        assert!(
            after - before > 44_100,
            "the output was starved while the resolver blocked: {before} -> {after}"
        );
    }

    #[test]
    fn gapless_no_drain_between_same_spec() {
        let h = harness(
            vec![
                fixture_track("a", "gap_a.wav"),
                fixture_track("b", "gap_b.wav"),
            ],
            FAST,
            None,
        );
        h.player.play(ids(&["a", "b"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), back_to_idle);
        assert_eq!(changes(&events), ids(&["a", "b"]));
        let first = events
            .iter()
            .position(|e| matches!(e, PlayerEvent::TrackChanged(_)))
            .unwrap();
        assert!(
            !events[first..]
                .iter()
                .any(|e| is_state(e, PlayerStatus::Loading)),
            "b was pre-opened and handed over, not loaded after a ended"
        );

        let mut expected = Vec::new();
        for name in ["gap_a.wav", "gap_b.wav"] {
            let mut source = open_source(&Input::File(fixtures().join(name))).unwrap();
            while source.read(&mut expected).unwrap() > 0 {}
        }
        let played = h.played.lock().unwrap().clone();
        assert_eq!(played.len(), expected.len(), "no padding, nothing lost");
        assert!(played == expected, "one continuous run across the boundary");
    }

    fn decode_all(name: &str) -> Vec<f32> {
        let mut samples = Vec::new();
        let mut source = open_source(&Input::File(fixtures().join(name))).unwrap();
        while source.read(&mut samples).unwrap() > 0 {}
        samples
    }

    #[test]
    fn replay_gain_scales_each_track_with_album_falling_back_to_track() {
        let mut a = fixture_track("a", "gap_a.wav");
        a.replay_gain_track_db = Some(-6.0);
        a.replay_gain_album_db = None; // album mode falls back to the track value
        let mut b = fixture_track("b", "gap_b.wav");
        b.replay_gain_track_db = Some(-1.0);
        b.replay_gain_album_db = Some(-12.0);
        let h = harness_options(
            vec![a, b],
            PlayerOptions {
                replay_gain: ReplayGain::Album,
                ..PlayerOptions::default()
            },
        );
        h.player.play(ids(&["a", "b"]), 0).unwrap();
        collect_until(&h.events, Duration::from_secs(10), back_to_idle);

        let first = decode_all("gap_a.wav");
        let second = decode_all("gap_b.wav");
        let played = h.played.lock().unwrap().clone();
        assert_eq!(played.len(), first.len() + second.len());
        let (gain_a, gain_b) = (10f32.powf(-6.0 / 20.0), 10f32.powf(-12.0 / 20.0));
        for (heard, decoded) in played.iter().zip(&first) {
            assert!((heard - decoded * gain_a).abs() < 1e-5);
        }
        for (heard, decoded) in played[first.len()..].iter().zip(&second) {
            assert!((heard - decoded * gain_b).abs() < 1e-5);
        }
    }

    #[test]
    fn replay_gain_off_and_untagged_tracks_play_unchanged() {
        let mut a = fixture_track("a", "gap_a.wav");
        a.replay_gain_track_db = Some(-6.0);
        let h = harness_options(vec![a], PlayerOptions::default());
        h.player.play(ids(&["a"]), 0).unwrap();
        collect_until(&h.events, Duration::from_secs(10), back_to_idle);
        assert!(*h.played.lock().unwrap() == decode_all("gap_a.wav"));
    }

    #[test]
    fn gapless_off_opens_the_next_track_after_the_end() {
        let h = harness_options(
            vec![
                fixture_track("a", "gap_a.wav"),
                fixture_track("b", "gap_b.wav"),
            ],
            PlayerOptions {
                gapless: false,
                ..PlayerOptions::default()
            },
        );
        h.player.play(ids(&["a", "b"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), back_to_idle);
        assert_eq!(changes(&events), ids(&["a", "b"]));
        let first = events
            .iter()
            .position(|e| matches!(e, PlayerEvent::TrackChanged(_)))
            .unwrap();
        assert!(
            events[first..]
                .iter()
                .any(|e| is_state(e, PlayerStatus::Loading)),
            "b was loaded after a ended, not pre-opened"
        );
    }

    #[test]
    fn starts_with_the_configured_volume() {
        let h = harness_options(
            Vec::new(),
            PlayerOptions {
                volume: 0.3,
                ..PlayerOptions::default()
            },
        );
        assert_eq!(h.player.state().volume, 0.3);
    }

    /// A slow first open (cold cache, a busy machine) must not cost the gapless hand-over:
    /// a real output does not bank the time it sat idle, so it cannot swallow the whole
    /// first track the moment it arrives.
    #[test]
    fn gapless_survives_a_slow_first_open() {
        let tracks = [
            fixture_track("a", "gap_a.wav"),
            fixture_track("b", "gap_b.wav"),
        ];
        let resolver: Resolver = Arc::new(move |id: &str| {
            if id == "a" {
                std::thread::sleep(Duration::from_millis(150));
            }
            tracks.iter().find(|track| track.id == id).cloned()
        });
        let h = harness_with(resolver, FAST, None);
        h.player.play(ids(&["a", "b"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), back_to_idle);
        assert_eq!(changes(&events), ids(&["a", "b"]));
        let first = events
            .iter()
            .position(|e| matches!(e, PlayerEvent::TrackChanged(_)))
            .unwrap();
        assert!(
            !events[first..]
                .iter()
                .any(|e| is_state(e, PlayerStatus::Loading)),
            "b was pre-opened and handed over, not loaded after a ended"
        );
        let played = h.played.lock().unwrap().clone();
        let expected: Vec<f32> = [decode_all("gap_a.wav"), decode_all("gap_b.wav")].concat();
        assert!(played == expected, "one continuous run across the boundary");
    }

    #[test]
    fn state_serializes_for_the_frontend() {
        let state = PlayerState {
            status: PlayerStatus::Loading,
            track: None,
            position_ms: 5,
            duration_ms: 6,
            volume: 0.5,
            queue_index: 1,
            queue_length: 2,
        };
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(value["status"], "loading");
        assert_eq!(value["positionMs"], 5);
        assert_eq!(value["queueLength"], 2);
        let error = serde_json::to_value(PlayerError {
            track_id: "t".into(),
            message: "m".into(),
        })
        .unwrap();
        assert_eq!(error["trackId"], "t");
    }

    #[test]
    fn starts_idle_at_default_volume_and_clamps_volume() {
        let h = harness(Vec::new(), FAST, None);
        let state = h.player.state();
        assert_eq!(state.status, PlayerStatus::Idle);
        assert_eq!(state.volume, 0.8);
        h.player.set_volume(1.7).unwrap();
        wait_state(&h.player, |s| s.volume == 1.0);
        h.player.set_volume(-1.0).unwrap();
        wait_state(&h.player, |s| s.volume == 0.0);
    }

    #[test]
    fn unknown_track_reports_an_error_and_goes_idle() {
        let h = harness(Vec::new(), FAST, None);
        h.player.play(ids(&["missing"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(5), |seen| {
            !errors(seen).is_empty()
                && seen
                    .iter()
                    .skip_while(|e| !matches!(e, PlayerEvent::Error(_)))
                    .any(|e| is_state(e, PlayerStatus::Idle))
        });
        assert_eq!(errors(&events), ids(&["missing"]));
    }
}
