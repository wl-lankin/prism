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
use crate::flow::FlowVariant;
use crate::library::model::Track;
use crate::settings::{Crossfade, ReplayGain};

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
    /// Bumped whenever the queue's ids change (not when only the position moves).
    pub queue_version: u64,
    /// The Flow this queue was dealt for; `None` (`null`) outside Flow.
    pub flow: Option<FlowVariant>,
}

/// One entry of `player_queue_range`. `track` is `None` when the id no longer resolves.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItem {
    pub index: u32,
    pub track: Option<Track>,
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
/// How long stopping the engine waits for the decode threads to drop their sources.
const SHUTDOWN_WAIT: Duration = Duration::from_millis(200);
/// After this many tracks in a row fail, the player gives up and goes idle.
pub const MAX_FAILURES_IN_A_ROW: u32 = 10;
/// The opener's workers: one per slot, so a slow pre-open never delays the track to play now.
const OPENER_CURRENT_THREAD: &str = "prism-opener-current";
const OPENER_NEXT_THREAD: &str = "prism-opener-next";

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
            queue_version: 0,
            flow: None,
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
    Play(Vec<String>, usize, Option<FlowVariant>),
    Append(Vec<String>),
    Jump(usize),
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

/// Clones the value behind `lock`, even when a panicking holder poisoned it.
fn read_lock<T: Clone>(lock: &Mutex<T>) -> T {
    match lock.lock() {
        Ok(value) => value.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

fn write_lock<T>(lock: &Mutex<T>, value: T) {
    match lock.lock() {
        Ok(mut stored) => *stored = value,
        Err(poisoned) => *poisoned.into_inner() = value,
    }
}

/// A cheap, cloneable way to drive the player from other threads (Flow, tray, shortcuts,
/// "Open with"). Like [`Player`], every method only sends a command or reads what the engine
/// last published. It does not keep the engine alive: once the [`Player`] is dropped, sends
/// fail with a message.
#[derive(Clone)]
pub struct PlayerHandle {
    commands: Sender<Command>,
    state: Arc<Mutex<PlayerState>>,
    queue: Arc<Mutex<Arc<Vec<String>>>>,
}

impl PlayerHandle {
    fn send(&self, command: Command) -> Result<(), String> {
        self.commands
            .send(command)
            .map_err(|_| "The audio engine is not running.".to_string())
    }

    /// Replaces the queue with `ids` and starts playing at `start`. Ends a Flow.
    pub fn play(&self, ids: Vec<String>, start: usize) -> Result<(), String> {
        self.play_with(ids, start, None)
    }

    /// Like [`PlayerHandle::play`], marking the new queue as dealt for `flow` (published as
    /// `PlayerState::flow` until the next play).
    pub fn play_with(
        &self,
        ids: Vec<String>,
        start: usize,
        flow: Option<FlowVariant>,
    ) -> Result<(), String> {
        self.send(Command::Play(ids, start, flow))
    }

    /// Adds `ids` at the end of the queue; what plays now keeps playing (a Flow too).
    pub fn append(&self, ids: Vec<String>) -> Result<(), String> {
        self.send(Command::Append(ids))
    }

    /// Plays the queue entry at `index`; an index out of range is ignored.
    pub fn jump(&self, index: usize) -> Result<(), String> {
        self.send(Command::Jump(index))
    }

    /// The latest state, as the engine published it.
    pub fn state(&self) -> PlayerState {
        read_lock(&self.state)
    }

    /// The queue's ids, as the engine published them.
    pub fn queue_ids(&self) -> Vec<String> {
        read_lock(&self.queue).as_ref().clone()
    }

    fn queue_snapshot(&self) -> Arc<Vec<String>> {
        read_lock(&self.queue)
    }
}

/// Handle to the engine thread. Every method only sends a command; none of them blocks on
/// audio work. Dropping it stops the engine.
pub struct Player {
    handle: PlayerHandle,
    resolver: Resolver,
    thread: Option<JoinHandle<()>>,
    /// The crossfade setting; kept for now, the engine does not blend tracks yet.
    crossfade: Mutex<(Crossfade, u32)>,
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
        let handle = PlayerHandle {
            commands,
            state: Arc::new(Mutex::new(PlayerState::idle(options.volume))),
            queue: Arc::new(Mutex::new(Arc::new(Vec::new()))),
        };
        let shared = Shared {
            state: Arc::clone(&handle.state),
            queue: Arc::clone(&handle.queue),
        };
        let engine_resolver = Arc::clone(&resolver);
        let thread = std::thread::Builder::new()
            .name("prism-player".into())
            .spawn(move || {
                let run = catch_unwind(AssertUnwindSafe(|| {
                    match Engine::new(factory, engine_resolver, sink, opener, shared, options) {
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
            handle,
            resolver,
            thread,
            crossfade: Mutex::new((Crossfade::Off, 0)),
        }
    }

    /// A cloneable handle for other threads.
    pub fn handle(&self) -> PlayerHandle {
        self.handle.clone()
    }

    fn send(&self, command: Command) -> Result<(), String> {
        self.handle.send(command)
    }

    /// Replaces the queue with `ids` and starts playing at `start`.
    pub fn play(&self, ids: Vec<String>, start: usize) -> Result<(), String> {
        self.handle.play(ids, start)
    }

    /// Adds `ids` at the end of the queue; what plays now keeps playing.
    pub fn append(&self, ids: Vec<String>) -> Result<(), String> {
        self.handle.append(ids)
    }

    /// Plays the queue entry at `index`; an index out of range is ignored.
    pub fn jump(&self, index: usize) -> Result<(), String> {
        self.handle.jump(index)
    }

    /// The queue's ids, as the engine published them.
    pub fn queue_ids(&self) -> Vec<String> {
        self.handle.queue_ids()
    }

    /// Up to `limit` queue entries from `offset`, resolved on the calling thread (the resolver
    /// may wait on the library DB, so call it off the UI thread).
    pub fn queue_range(&self, offset: u32, limit: u32) -> Vec<QueueItem> {
        let ids = self.handle.queue_snapshot();
        ids.iter()
            .enumerate()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|(index, id)| QueueItem {
                index: index as u32,
                track: (self.resolver)(id),
            })
            .collect()
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

    /// Stores the crossfade mode and length (ms). Has no effect on playback yet.
    pub fn set_crossfade(&self, mode: Crossfade, ms: u32) -> Result<(), String> {
        write_lock(&self.crossfade, (mode, ms));
        Ok(())
    }

    /// The latest state, as the engine published it.
    pub fn state(&self) -> PlayerState {
        self.handle.state()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.handle.send(Command::Shutdown);
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

/// The opener's job queues, one worker per slot: a slow pre-open of the next track never
/// delays opening the track to play now (a manual next, a jump).
struct Openers {
    current: Sender<OpenJob>,
    next: Sender<OpenJob>,
}

impl Openers {
    fn spawn(
        resolver: Resolver,
        opener: Opener,
        generation: Arc<AtomicU64>,
        results: Sender<Opened>,
    ) -> Openers {
        Openers {
            current: spawn_opener(
                OPENER_CURRENT_THREAD,
                Arc::clone(&resolver),
                Arc::clone(&opener),
                Arc::clone(&generation),
                results.clone(),
            ),
            next: spawn_opener(OPENER_NEXT_THREAD, resolver, opener, generation, results),
        }
    }

    fn send(&self, job: OpenJob) -> Result<(), String> {
        let jobs = match job.slot {
            Slot::Current => &self.current,
            Slot::Next => &self.next,
        };
        jobs.send(job)
            .map_err(|_| "the file opener is not running".to_string())
    }
}

/// Resolves and opens tracks off the engine thread: the resolver can wait on the library DB
/// and an open decodes the first packet (FFmpeg can take seconds). The engine thread never
/// touches either. Jobs from an older generation are skipped.
fn spawn_opener(
    name: &str,
    resolver: Resolver,
    opener: Opener,
    generation: Arc<AtomicU64>,
    results: Sender<Opened>,
) -> Sender<OpenJob> {
    let (jobs, job_rx) = crossbeam_channel::unbounded::<OpenJob>();
    let spawned = std::thread::Builder::new()
        .name(name.into())
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

/// What the engine publishes for [`PlayerHandle`] readers.
struct Shared {
    state: Arc<Mutex<PlayerState>>,
    queue: Arc<Mutex<Arc<Vec<String>>>>,
}

/// Lives on the engine thread and owns everything that is not `Send`.
struct Engine {
    playback: Playback,
    queue: Queue,
    /// Bumped whenever the queue's ids change.
    queue_version: u64,
    /// The Flow the queue was dealt for; set by every play, kept by appends.
    flow: Option<FlowVariant>,
    sink: EventSink,
    shared: Shared,
    openers: Openers,
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
    /// When the user paused; a pending hand-over's deadline moves by the paused time.
    paused_at: Option<Instant>,
    /// The queue ran out; idle once the output has played everything.
    ending: bool,
    /// Tracks that failed in a row since one was last heard.
    failures: u32,
    dirty: bool,
    last_emit: Instant,
    /// The last `player://state` sent; an unchanged snapshot is not sent again.
    last_sent: Option<PlayerState>,
}

impl Engine {
    fn new(
        factory: Arc<dyn OutputFactory>,
        resolver: Resolver,
        sink: EventSink,
        opener: Opener,
        shared: Shared,
        options: PlayerOptions,
    ) -> anyhow::Result<Engine> {
        let mut playback = Playback::new(factory, options.device.clone())?;
        playback.set_volume(options.volume);
        if let Some(err) = playback.output_error() {
            eprintln!("no audio output yet: {err}");
        }
        let generation = Arc::new(AtomicU64::new(0));
        let (results, opened) = crossbeam_channel::unbounded();
        let openers = Openers::spawn(resolver, opener, Arc::clone(&generation), results);
        Ok(Engine {
            playback,
            queue: Queue::new(),
            queue_version: 0,
            flow: None,
            sink,
            shared,
            openers,
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
            paused_at: None,
            ending: false,
            failures: 0,
            dirty: true,
            last_emit: Instant::now(),
            last_sent: None,
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
        // Lets the decode threads drop their sources (killing FFmpeg children) before the
        // process may exit; a read stuck longer than this is left behind.
        self.playback.shutdown(SHUTDOWN_WAIT);
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
            Command::Play(ids, start, flow) => {
                // A pending hand-over is settled at the index it belongs to.
                self.settle_handover();
                self.flow = flow;
                self.queue.set(ids, start);
                self.queue_changed();
                self.failures = 0;
                self.start_current(true);
            }
            Command::Append(ids) => self.append(ids),
            Command::Jump(index) => {
                self.settle_handover();
                if self.queue.jump(index) {
                    self.failures = 0;
                    self.start_current(true);
                }
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

    /// The queue's ids changed: bump the version and publish the ids for handle readers.
    fn queue_changed(&mut self) {
        self.queue_version += 1;
        write_lock(&self.shared.queue, Arc::new(self.queue.ids().to_vec()));
        self.dirty = true;
    }

    /// Adds `ids` at the end. Playback goes on; when only a tail was left, the first appended
    /// track follows it (even when the queue gave up on failed tracks before the end).
    fn append(&mut self, ids: Vec<String>) {
        if ids.is_empty() {
            return;
        }
        let first = self.queue.len();
        self.queue.extend(ids);
        self.queue_changed();
        if self.ending && self.queue.jump(first) {
            self.failures = 0;
            self.start_current(false);
        }
    }

    fn toggle(&mut self) {
        match self.status {
            PlayerStatus::Playing => {
                self.playback.set_paused(true);
                self.status = PlayerStatus::Paused;
                self.paused_at = Some(Instant::now());
            }
            PlayerStatus::Paused => {
                self.playback.set_paused(false);
                self.status = PlayerStatus::Playing;
                // Nothing was heard while paused, so the hand-over is that much later.
                if let Some(paused_at) = self.paused_at.take() {
                    if let Some((at, _)) = self.handover.as_mut() {
                        *at += paused_at.elapsed();
                    }
                }
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
        if self.ending {
            // Only a tail is left (no longer decoding, so it cannot be seeked): start the
            // audible track again. The queue may already stand on a later track that failed.
            if self.shown.is_some() {
                self.queue.jump(self.shown_index);
            }
            self.start_current(true);
        } else if self.seekable() && self.position_ms() > RESTART_AFTER_MS {
            self.seek(0);
        } else if self.queue.back() {
            self.start_current(true);
        } else if self.seekable() {
            self.seek(0);
        }
    }

    /// A seek that changes nothing (not seekable, or to where it already is) is ignored.
    fn seek(&mut self, ms: u64) {
        if !self.seekable() {
            return;
        }
        self.settle_handover();
        let duration = self.current.as_ref().map_or(0, |track| track.duration_ms);
        let ms = if duration > 0 { ms.min(duration) } else { ms };
        if ms == self.position_ms() {
            return;
        }
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
        self.paused_at = None;
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
        if let Err(message) = self.openers.send(job) {
            self.error(&id, &message);
            // Every other track would fail the same way.
            self.finish(cut);
        }
    }

    /// Moves past a track that failed (or ended): the next one, or the end of the queue.
    /// After [`MAX_FAILURES_IN_A_ROW`] failures the player gives up and goes idle.
    fn skip(&mut self, cut: bool) {
        if self.failures >= MAX_FAILURES_IN_A_ROW {
            self.finish(cut);
        } else if self.queue.advance() {
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
        self.paused_at = None;
        self.ending = false;
        self.start_paused = false;
        self.failures = 0;
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
            // Everything was heard, including a hand-over whose deadline has not come yet
            // (a track shorter than what was buffered ahead of it).
            self.settle_handover();
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

    /// Opens the next track once the current one is within [`PRELOAD_MS`] of its end, or
    /// once it has been decoded to its end. The second rule needs no duration, so tracks
    /// whose length the library does not know (often FFmpeg-decoded ones) are gapless too.
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
        let near_end = track.duration_ms > 0
            && track
                .duration_ms
                .saturating_sub(self.playback.position_ms())
                <= PRELOAD_MS;
        if !near_end && !self.playback.current_exhausted() {
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
        self.preload = match self.openers.send(job) {
            Ok(()) => Preload::Requested(index),
            Err(message) => Preload::Failed(index, id, message),
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
        // A hand-over to the failing track was never heard: the previous track's tail is
        // still playing and is not cut.
        let unheard = self.handover.take().is_some();
        self.error(&id, &format!("{err:#}"));
        self.decoding = false;
        if unheard {
            self.playback.drop_sources();
        } else {
            self.playback.stop();
        }
        self.skip(!unheard);
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
        self.failures = 0;
        self.dirty = true;
        (self.sink)(PlayerEvent::TrackChanged(track));
    }

    /// Reports a track that could not be played. The one that makes it
    /// [`MAX_FAILURES_IN_A_ROW`] is reported as that instead (the player then gives up), and
    /// nothing after it is reported, so a dead queue does not flood the UI.
    fn error(&mut self, track_id: &str, message: &str) {
        self.dirty = true;
        self.failures += 1;
        let message = match self.failures {
            n if n < MAX_FAILURES_IN_A_ROW => message.to_string(),
            MAX_FAILURES_IN_A_ROW => {
                format!("{MAX_FAILURES_IN_A_ROW} tracks in a row could not be played")
            }
            _ => return,
        };
        (self.sink)(PlayerEvent::Error(PlayerError {
            track_id: track_id.to_string(),
            message,
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
            queue_version: self.queue_version,
            flow: self.flow.clone(),
        }
    }

    /// When something may have changed, or every [`STATE_INTERVAL`] while playing: updates
    /// the shared state (what `player_state` returns) and emits it, unless it is the same as
    /// the last one sent (the periodic update while playing is always sent).
    fn publish(&mut self) {
        let periodic =
            self.status == PlayerStatus::Playing && self.last_emit.elapsed() >= STATE_INTERVAL;
        if !self.dirty && !periodic {
            return;
        }
        self.dirty = false;
        let state = self.snapshot();
        write_lock(&self.shared.state, state.clone());
        if !periodic && self.last_sent.as_ref() == Some(&state) {
            return;
        }
        self.last_emit = Instant::now();
        self.last_sent = Some(state.clone());
        (self.sink)(PlayerEvent::State(state));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use crossbeam_channel::{Receiver, Sender};

    use super::*;
    use crate::audio::output::BUFFER_MS;
    use crate::audio::readahead::tests::SlowSource;
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

    /// What [`BufferedSink`] changes as time passes.
    struct BufferedState {
        last: Instant,
        /// Frames the device may play but has not yet (a fraction carried between calls).
        due: f64,
        buffer: VecDeque<f32>,
        paused: bool,
    }

    /// An output like the real one: a ring of `ring_frames` that the device drains at `speed`
    /// times real time, only while not paused. `queued()` is what is left in the ring,
    /// `clear()` discards it without playing it, and every sample played is recorded.
    struct BufferedSink {
        spec: StreamSpec,
        speed: f64,
        ring_frames: usize,
        state: RefCell<BufferedState>,
        played: Arc<Mutex<Vec<f32>>>,
        events: Receiver<OutputEvent>,
        _keep: Sender<OutputEvent>,
    }

    impl BufferedSink {
        /// Plays what the device has consumed since the last call.
        fn drain(&self) {
            let channels = self.spec.channels as usize;
            let mut state = self.state.borrow_mut();
            let now = Instant::now();
            let elapsed = now.duration_since(state.last).as_secs_f64();
            state.last = now;
            if state.paused {
                return;
            }
            state.due += elapsed * self.speed * self.spec.sample_rate as f64;
            let frames = (state.due as usize).min(state.buffer.len() / channels);
            state.due -= frames as f64;
            let samples: Vec<f32> = state.buffer.drain(..frames * channels).collect();
            if state.buffer.is_empty() {
                // A device does not bank the time it had nothing to play.
                state.due = 0.0;
            }
            self.played.lock().unwrap().extend(samples);
        }
    }

    impl OutputSink for BufferedSink {
        fn spec(&self) -> StreamSpec {
            self.spec
        }
        fn push(&mut self, samples: &[f32]) -> usize {
            self.drain();
            let channels = self.spec.channels as usize;
            let mut state = self.state.borrow_mut();
            let room = self.ring_frames - state.buffer.len() / channels;
            let taken = room.min(samples.len() / channels) * channels;
            state.buffer.extend(&samples[..taken]);
            taken
        }
        fn queued(&self) -> usize {
            self.drain();
            self.state.borrow().buffer.len()
        }
        fn clear(&mut self) {
            self.drain();
            self.state.borrow_mut().buffer.clear();
        }
        fn set_volume(&mut self, _volume: f32) {}
        fn set_paused(&mut self, paused: bool) {
            self.drain();
            self.state.borrow_mut().paused = paused;
        }
        fn errors(&self) -> Receiver<OutputEvent> {
            self.events.clone()
        }
    }

    struct BufferedFactory {
        speed: f64,
        ring_ms: u64,
        played: Arc<Mutex<Vec<f32>>>,
    }

    impl OutputFactory for BufferedFactory {
        fn open(&self, _device: Option<String>) -> anyhow::Result<Box<dyn OutputSink>> {
            let (tx, rx) = crossbeam_channel::unbounded();
            Ok(Box::new(BufferedSink {
                spec: STEREO_44K,
                speed: self.speed,
                ring_frames: (self.ring_ms * 44_100 / 1000) as usize,
                state: RefCell::new(BufferedState {
                    last: Instant::now(),
                    due: 0.0,
                    buffer: VecDeque::new(),
                    paused: false,
                }),
                played: Arc::clone(&self.played),
                events: rx,
                _keep: tx,
            }))
        }
    }

    /// `ms` of a constant `value` at the output spec, so the output shows which track played.
    struct Tone {
        value: f32,
        ms: u64,
        frames_left: u64,
    }

    impl Tone {
        fn new(value: f32, ms: u64) -> Tone {
            Tone {
                value,
                ms,
                frames_left: ms * 44_100 / 1000,
            }
        }
    }

    impl AudioSource for Tone {
        fn spec(&self) -> StreamSpec {
            STEREO_44K
        }
        fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize> {
            let frames = self.frames_left.min(1024) as usize;
            out.extend(std::iter::repeat_n(self.value, frames * 2));
            self.frames_left -= frames as u64;
            Ok(frames)
        }
        fn seek(&mut self, ms: u64) -> anyhow::Result<()> {
            self.frames_left = self.ms.saturating_sub(ms) * 44_100 / 1000;
            Ok(())
        }
        fn duration_ms(&self) -> Option<u64> {
            Some(self.ms)
        }
    }

    /// Opens fine, fails on the first read.
    struct Broken;

    impl AudioSource for Broken {
        fn spec(&self) -> StreamSpec {
            STEREO_44K
        }
        fn read(&mut self, _out: &mut Vec<f32>) -> anyhow::Result<usize> {
            anyhow::bail!("broken stream")
        }
        fn seek(&mut self, _ms: u64) -> anyhow::Result<()> {
            Ok(())
        }
        fn duration_ms(&self) -> Option<u64> {
            None
        }
    }

    /// The tone values the buffered tests use: "a" and "c" play, "broken" fails on read.
    const TONE_A: f32 = 0.25;
    const TONE_C: f32 = 0.5;
    /// "a" is longer than the ring, so it is still decoding when the next track is
    /// pre-opened; once it is decoded, a full ring (2 s at real time) is left to play.
    const TONE_A_MS: u64 = 2_500;
    const BUFFERED_RING_MS: u64 = 2_000;

    fn buffered_harness() -> Harness {
        let tracks: Vec<Track> = ["a", "broken", "c"]
            .iter()
            .map(|id| {
                let mut track = fixture_track(id, "tagged.flac");
                track.duration_ms = if *id == "broken" { 0 } else { TONE_A_MS };
                track
            })
            .collect();
        let resolver: Resolver =
            Arc::new(move |id: &str| tracks.iter().find(|track| track.id == id).cloned());
        let opener: Opener = Arc::new(|track: &Track, _spec: StreamSpec| {
            Ok(match track.id.as_str() {
                "a" => Box::new(Tone::new(TONE_A, TONE_A_MS)) as Box<dyn AudioSource>,
                "c" => Box::new(Tone::new(TONE_C, TONE_A_MS)),
                _ => Box::new(Broken),
            })
        });
        let played = Arc::new(Mutex::new(Vec::new()));
        let factory = Arc::new(BufferedFactory {
            speed: 1.0,
            ring_ms: BUFFERED_RING_MS,
            played: Arc::clone(&played),
        });
        let (tx, events) = crossbeam_channel::unbounded();
        let sink: EventSink = Arc::new(move |event| {
            let _ = tx.send(event);
        });
        let player =
            Player::spawn_with_options(factory, resolver, sink, opener, PlayerOptions::default());
        Harness {
            player,
            events,
            played,
        }
    }

    /// Samples of `value` that reached the device.
    fn played_of(played: &Mutex<Vec<f32>>, value: f32) -> usize {
        played
            .lock()
            .unwrap()
            .iter()
            .filter(|sample| (**sample - value).abs() < 1e-6)
            .count()
    }

    /// Every sample of "a" (stereo).
    const TONE_A_SAMPLES: usize = (TONE_A_MS * 44_100 / 1000) as usize * 2;

    #[test]
    fn failing_preload_keeps_the_tail() {
        let h = buffered_harness();
        h.player.play(ids(&["a", "broken"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(20), back_to_idle);
        assert_eq!(errors(&events), ids(&["broken"]));
        assert_eq!(changes(&events), ids(&["a"]));
        assert_eq!(
            played_of(&h.played, TONE_A),
            TONE_A_SAMPLES,
            "all of a was heard: the failure after the hand-over did not cut its tail"
        );
    }

    #[test]
    fn prev_in_the_tail_after_a_failed_last_track_restarts_the_audible_one() {
        let h = buffered_harness();
        h.player.play(ids(&["a", "broken"]), 0).unwrap();
        collect_until(&h.events, Duration::from_secs(20), |seen| {
            !errors(seen).is_empty()
        });
        // The queue stands on "broken"; "a"'s tail is still playing.
        h.player.prev().unwrap();
        let events = collect_until(&h.events, Duration::from_secs(20), |seen| {
            !changes(seen).is_empty()
        });
        assert_eq!(changes(&events), ids(&["a"]), "a starts again");
        assert!(
            errors(&events).is_empty(),
            "broken was not retried: {events:#?}"
        );
        let state = wait_state(&h.player, |s| s.status == PlayerStatus::Playing);
        assert_eq!(state.queue_index, 0);
        assert_eq!(state.track.unwrap().id, "a");
    }

    #[test]
    fn append_during_the_tail_continues_without_idle() {
        let h = buffered_harness();
        h.player.play(ids(&["a"]), 0).unwrap();
        collect_until(&h.events, Duration::from_secs(20), |seen| {
            changes(seen) == ids(&["a"])
        });
        // Once the device has played more than "a" minus a full ring, all of "a" has been
        // decoded: only its tail is left (about 1.9 s of it).
        let decoded = TONE_A_SAMPLES - (BUFFERED_RING_MS * 44_100 / 1000) as usize * 2;
        let deadline = Instant::now() + Duration::from_secs(20);
        while played_of(&h.played, TONE_A) <= decoded + 4_410 {
            assert!(Instant::now() < deadline, "a never played");
            std::thread::sleep(Duration::from_millis(5));
        }
        h.player.append(ids(&["c"])).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(20), |seen| {
            !changes(seen).is_empty()
        });
        assert_eq!(changes(&events), ids(&["c"]));
        assert!(
            events.iter().any(|e| is_state(e, PlayerStatus::Loading)),
            "c was loaded after a's end, not pre-opened: {events:#?}"
        );
        assert!(
            !events.iter().any(|e| is_state(e, PlayerStatus::Idle)),
            "no idle between a and c: {events:#?}"
        );
        assert_eq!(
            played_of(&h.played, TONE_A),
            TONE_A_SAMPLES,
            "a's tail played out before c"
        );
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
    const RING_MS: u32 = BUFFER_MS;

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

    /// FFmpeg-decoded tracks often have no duration in the library: gapless still works,
    /// because the next track is opened once the current one is fully decoded.
    #[test]
    fn unknown_duration_tracks_are_gapless() {
        let tracks: Vec<Track> = [("a", "gap_a.wav"), ("b", "gap_b.wav")]
            .iter()
            .map(|(id, file)| {
                let mut track = fixture_track(id, file);
                track.duration_ms = 0;
                track
            })
            .collect();
        // 5x: the pre-open starts once "a" is decoded, with up to 200 ms of it left to
        // play (in real time, up to the 3 s read-ahead).
        let h = harness(tracks, 5.0, None);
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
        assert_eq!(played.len(), expected.len(), "no padding, nothing lost");
        assert!(played == expected, "one continuous run across the boundary");
    }

    #[test]
    fn dropping_the_player_drops_the_decoded_sources() {
        let mut track = fixture_track("slow", "tagged.flac");
        track.duration_ms = 10_000;
        let drops = Arc::new(Mutex::new(Vec::new()));
        let opened = Arc::clone(&drops);
        let opener: Opener = Arc::new(move |_track: &Track, _spec: StreamSpec| {
            // Always in the middle of a 20 ms read: a stop that does not wait returns first.
            let mut source = SlowSource::new(10);
            source.read_delay = Duration::from_millis(20);
            opened.lock().unwrap().push(Arc::clone(&source.drops));
            Ok(Box::new(source) as Box<dyn AudioSource>)
        });
        let h = harness(vec![track], FAST, Some(opener));
        h.player.play(ids(&["slow"]), 0).unwrap();
        wait_state(&h.player, |s| s.status == PlayerStatus::Playing);
        drop(h.player);
        let drops = drops.lock().unwrap();
        assert_eq!(drops.len(), 1);
        assert_eq!(
            drops[0].load(Ordering::SeqCst),
            1,
            "the source (an FFmpeg child in real life) was dropped before the drop returned"
        );
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
            queue_version: 7,
            flow: None,
        };
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(value["status"], "loading");
        assert_eq!(value["positionMs"], 5);
        assert_eq!(value["queueLength"], 2);
        assert_eq!(value["queueVersion"], 7);
        assert!(value["flow"].is_null(), "null outside Flow");
        let flowing = PlayerState {
            flow: Some(FlowVariant::Genre {
                genre: "Jazz".into(),
            }),
            ..state
        };
        let value = serde_json::to_value(&flowing).unwrap();
        assert_eq!(value["flow"]["kind"], "genre");
        assert_eq!(value["flow"]["genre"], "Jazz");
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

    /// An opener that plays [`SILENCE_MS`] of silence for every track.
    fn silence_opener() -> Opener {
        Arc::new(|_track: &Track, _spec: StreamSpec| {
            Ok(Box::new(Silence {
                frames_left: SILENCE_MS * 44_100 / 1000,
            }) as Box<dyn AudioSource>)
        })
    }

    /// Tracks `ids` that last [`SILENCE_MS`] (pair with [`silence_opener`]).
    fn long_tracks(ids: &[&str]) -> Vec<Track> {
        ids.iter()
            .map(|id| {
                let mut track = fixture_track(id, "tagged.flac");
                track.duration_ms = SILENCE_MS;
                track
            })
            .collect()
    }

    fn states(events: &[PlayerEvent]) -> Vec<&PlayerState> {
        events
            .iter()
            .filter_map(|event| match event {
                PlayerEvent::State(state) => Some(state),
                _ => None,
            })
            .collect()
    }

    fn playing_track(state: &PlayerState, id: &str) -> bool {
        state.status == PlayerStatus::Playing && state.track.as_ref().is_some_and(|t| t.id == id)
    }

    #[test]
    fn append_keeps_playing_and_bumps_version() {
        let h = harness(long_tracks(&["one", "two"]), 1.0, Some(silence_opener()));
        h.player.play(ids(&["one"]), 0).unwrap();
        let before = wait_state(&h.player, |s| playing_track(s, "one"));
        assert_eq!(h.player.handle().queue_ids(), ids(&["one"]));

        // Other threads (Flow, tray, shortcuts) use the handle.
        let handle = h.player.handle();
        std::thread::spawn(move || handle.append(ids(&["two"])).unwrap())
            .join()
            .unwrap();
        let after = wait_state(&h.player, |s| s.queue_length == 2);
        assert!(after.queue_version > before.queue_version);
        assert!(playing_track(&after, "one"), "still playing: {after:#?}");
        assert_eq!(after.queue_index, 0);
        assert_eq!(h.player.handle().queue_ids(), ids(&["one", "two"]));
        assert_eq!(h.player.handle().state(), h.player.state());

        // Appending nothing changes nothing.
        h.player.append(Vec::new()).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(h.player.state().queue_version, after.queue_version);
    }

    #[test]
    fn jump_plays_that_index() {
        let h = harness(long_tracks(&["a", "b", "c"]), 1.0, Some(silence_opener()));
        h.player.play(ids(&["a", "b", "c"]), 0).unwrap();
        let before = wait_state(&h.player, |s| playing_track(s, "a"));
        h.player.handle().jump(2).unwrap();
        let state = wait_state(&h.player, |s| playing_track(s, "c"));
        assert_eq!(state.queue_index, 2);
        assert_eq!(
            state.queue_version, before.queue_version,
            "the ids did not change"
        );

        // Out of range: ignored.
        h.player.jump(7).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let state = h.player.state();
        assert!(playing_track(&state, "c"));
        assert_eq!(state.queue_index, 2);
    }

    #[test]
    fn queue_range_marks_missing_tracks() {
        let h = harness(long_tracks(&["a", "b"]), 1.0, Some(silence_opener()));
        h.player.play(ids(&["a", "gone", "b"]), 0).unwrap();
        wait_state(&h.player, |s| s.queue_length == 3);
        let items = h.player.queue_range(1, 10);
        let summary: Vec<(u32, Option<String>)> = items
            .iter()
            .map(|item| (item.index, item.track.as_ref().map(|t| t.id.clone())))
            .collect();
        assert_eq!(
            summary,
            vec![(1, None), (2, Some("b".to_string()))],
            "the range starts at the offset and marks ids that no longer resolve"
        );
        assert_eq!(h.player.queue_range(0, 1).len(), 1);
        assert!(h.player.queue_range(3, 10).is_empty());
        let value = serde_json::to_value(&items[1]).unwrap();
        assert_eq!(value["index"], 2);
        assert_eq!(value["track"]["id"], "b");
    }

    #[test]
    fn slow_preload_does_not_delay_next() {
        let mut tracks = long_tracks(&["a", "b", "c"]);
        // "a" claims to be short, so "b" is pre-opened as soon as "a" plays.
        tracks[0].duration_ms = 1_000;
        let (preloading_tx, preloading) = crossbeam_channel::unbounded();
        let opener: Opener = Arc::new(move |_track: &Track, _spec: StreamSpec| {
            if std::thread::current().name() == Some(OPENER_NEXT_THREAD) {
                let _ = preloading_tx.send(());
                std::thread::sleep(Duration::from_secs(2));
            }
            Ok(Box::new(Silence {
                frames_left: SILENCE_MS * 44_100 / 1000,
            }) as Box<dyn AudioSource>)
        });
        let h = harness(tracks, 1.0, Some(opener));
        h.player.play(ids(&["a", "b", "c"]), 0).unwrap();
        preloading
            .recv_timeout(Duration::from_secs(5))
            .expect("the next track is pre-opened");
        let asked = Instant::now();
        h.player.next().unwrap();
        collect_until(&h.events, Duration::from_secs(5), |seen| {
            states(seen).iter().any(|s| playing_track(s, "b"))
        });
        assert!(
            asked.elapsed() < Duration::from_millis(500),
            "next waited for the pre-open: {:?}",
            asked.elapsed()
        );
    }

    #[test]
    fn dead_queue_reports_once() {
        let h = harness(Vec::new(), FAST, None);
        let missing: Vec<String> = (0..50).map(|n| format!("missing{n}")).collect();
        h.player.play(missing, 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), |seen| {
            let Some(last_error) = seen
                .iter()
                .rposition(|e| matches!(e, PlayerEvent::Error(_)))
            else {
                return false;
            };
            seen[last_error..]
                .iter()
                .any(|e| is_state(e, PlayerStatus::Idle))
        });
        let mut events = events;
        std::thread::sleep(Duration::from_millis(300));
        events.extend(h.events.try_iter());
        let messages: Vec<String> = events
            .iter()
            .filter_map(|event| match event {
                PlayerEvent::Error(error) => Some(error.message.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            messages.len(),
            10,
            "one per failed track, then none: {messages:#?}"
        );
        assert_eq!(
            messages.last().map(String::as_str),
            Some("10 tracks in a row could not be played")
        );
        assert_eq!(h.player.state().status, PlayerStatus::Idle);
    }

    #[test]
    fn noop_seek_is_silent() {
        let h = harness(long_tracks(&["a"]), FAST, Some(silence_opener()));
        // Idle: nothing to seek.
        h.player.seek(1_000).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let idle: Vec<PlayerEvent> = h.events.try_iter().collect();
        assert!(
            states(&idle).iter().all(|s| s.status == PlayerStatus::Idle) && idle.len() <= 1,
            "only the initial idle state: {idle:#?}"
        );

        h.player.play(ids(&["a"]), 0).unwrap();
        wait_state(&h.player, |s| s.status == PlayerStatus::Playing);
        h.player.toggle().unwrap();
        wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
        h.player.seek(4_000).unwrap();
        wait_state(&h.player, |s| s.position_ms == 4_000);
        std::thread::sleep(Duration::from_millis(100));
        h.events.try_iter().for_each(drop);

        h.player.seek(4_000).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let later: Vec<PlayerEvent> = h.events.try_iter().collect();
        assert!(
            later.is_empty(),
            "a seek to the same spot is silent: {later:#?}"
        );
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
