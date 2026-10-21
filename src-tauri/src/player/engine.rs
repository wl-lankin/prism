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
    is_offline, open_source_with, AudioSource, Input, NextOptions, OutputFactory, Playback,
    PumpStatus, StreamSpec, READ_AHEAD_MS,
};
use crate::flow::FlowVariant;
use crate::library::model::{Source, Track};
use crate::navidrome::stars::ID_PREFIX as NAVIDROME_PREFIX;
use crate::radio::model::RADIO_PREFIX;
use crate::settings::{Crossfade, RepeatMode, ReplayGain};

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
    /// The chosen mode; a Flow ignores it (and keeps it for the next queue).
    pub repeat: RepeatMode,
    /// The upcoming part plays in random order; a Flow ignores it too.
    pub shuffle: bool,
}

/// One entry of `player_queue_range`. `track` is `None` when the id no longer resolves.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItem {
    pub index: u32,
    pub track: Option<Track>,
}

/// What kind of failure a [`PlayerError`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ErrorKind {
    /// The track could not be opened or decoded; the player moved on.
    Track,
    /// The server could not be reached; the player paused on the track (see
    /// [`OFFLINE_MESSAGE`]) and Play tries it again from where it stopped.
    Offline,
}

/// Payload of `player://error`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerError {
    pub track_id: String,
    pub message: String,
    pub kind: ErrorKind,
    /// Where the entry comes from: the UI words an offline notice per source.
    pub source: Source,
}

/// The message of an [`ErrorKind::Offline`] error of a Navidrome track.
pub const OFFLINE_MESSAGE: &str = "Navidrome is not reachable";
/// The message of an [`ErrorKind::Offline`] error of a station.
pub const STATION_OFFLINE_MESSAGE: &str = "The station is not reachable";

/// A radio station: no length, no seek, nothing before or after it is blended in.
pub fn is_live(track: &Track) -> bool {
    track.source == Source::Radio && track.duration_ms == 0
}

/// The source of an entry, from its track or (not resolved) from its id.
fn source_of(track: Option<&Track>, id: &str) -> Source {
    match track {
        Some(track) => track.source,
        None if id.starts_with(RADIO_PREFIX) => Source::Radio,
        None if id.starts_with(NAVIDROME_PREFIX) => Source::Navidrome,
        None => Source::Local,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    /// The state changed (or 250 ms passed while playing).
    State(PlayerState),
    /// A track became audible.
    TrackChanged(Track),
    /// A track could not be opened or decoded; the player moves on. Or (`ErrorKind::Offline`)
    /// the server could not be reached; the player pauses on that track.
    Error(PlayerError),
    /// 0..=1, the loudness of what is handed to the output, every [`LEVEL_INTERVAL`] while
    /// playing. Not part of [`PlayerState`]: it neither updates nor repeats the state.
    Level(f32),
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
    /// When tracks blend into each other, and for how long (ms); see [`crossfade_for`].
    pub crossfade: Crossfade,
    pub crossfade_ms: u32,
    pub repeat: RepeatMode,
    pub shuffle: bool,
}

impl Default for PlayerOptions {
    /// Off for crossfade: the app passes the setting (whose default is Flow).
    fn default() -> Self {
        PlayerOptions {
            volume: DEFAULT_VOLUME,
            device: None,
            gapless: true,
            replay_gain: ReplayGain::Off,
            crossfade: Crossfade::Off,
            crossfade_ms: 0,
            repeat: RepeatMode::Off,
            shuffle: false,
        }
    }
}

/// How long `next` blends into `current` (ms): `Off` never; `Flow` only inside a Flow;
/// `Always` everywhere except into the following track of the same album, so albums stay
/// gapless.
pub fn crossfade_for(
    mode: Crossfade,
    ms: u32,
    in_flow: bool,
    current: &Track,
    next: &Track,
) -> u32 {
    match mode {
        Crossfade::Off => 0,
        Crossfade::Flow if in_flow => ms,
        Crossfade::Flow => 0,
        Crossfade::Always => {
            if album_goes_on(current, next) {
                0
            } else {
                ms
            }
        }
    }
}

/// `next` is where the album goes on after `current`: the same (known) album, and the
/// following track of the same disc or the first track of the next disc.
fn album_goes_on(current: &Track, next: &Track) -> bool {
    if current.album_id.is_empty() || current.album_id != next.album_id {
        return false;
    }
    let following = current.track_no.and_then(|no| no.checked_add(1));
    let same_disc = next.disc_no == current.disc_no;
    let next_disc = current.disc_no.and_then(|disc| disc.checked_add(1)) == next.disc_no
        && next.disc_no.is_some();
    (same_disc && following.is_some() && next.track_no == following)
        || (next_disc && next.track_no == Some(1))
}

/// The next track is opened this long before the current one ends, for gapless playback.
pub const PRELOAD_MS: u64 = 5_000;
/// `prev` restarts the current track once it has played longer than this.
pub const RESTART_AFTER_MS: u64 = 3_000;
/// `player://state` is repeated this often while playing.
const STATE_INTERVAL: Duration = Duration::from_millis(250);
/// [`PlayerEvent::Level`] is sent this often while playing.
pub const LEVEL_INTERVAL: Duration = Duration::from_millis(50);
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
    fn idle(options: &PlayerOptions) -> PlayerState {
        PlayerState {
            status: PlayerStatus::Idle,
            track: None,
            position_ms: 0,
            duration_ms: 0,
            volume: options.volume,
            queue_index: 0,
            queue_length: 0,
            queue_version: 0,
            flow: None,
            repeat: options.repeat,
            shuffle: options.shuffle,
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
    /// Pauses only what plays (or loads); never resumes.
    Pause,
    Next,
    Prev,
    Seek(u64),
    SetVolume(f32),
    SetDevice(Option<String>),
    SetGapless(bool),
    SetReplayGain(ReplayGain),
    SetCrossfade(Crossfade, u32),
    SetRepeat(RepeatMode),
    SetShuffle(bool),
    /// Moves the queue entry at the first index to the second.
    Move(usize, usize),
    Remove(usize),
    /// A fresher copy of a track (its cover changed): replaces the shown one when the ids match.
    UpdateTrack(Box<Track>),
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
/// when the album has none; a track without tags plays at 1.0, and so does a station.
pub fn track_gain(track: &Track, mode: ReplayGain) -> f32 {
    if is_live(track) {
        return 1.0;
    }
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

    /// Moves the queue entry at `from` to `to`; what plays keeps playing. Out of range is
    /// ignored.
    pub fn move_entry(&self, from: usize, to: usize) -> Result<(), String> {
        self.send(Command::Move(from, to))
    }

    /// Removes the queue entry at `index`. Removing the playing one plays the next one.
    pub fn remove(&self, index: usize) -> Result<(), String> {
        self.send(Command::Remove(index))
    }

    /// Off, all (the first track follows the last) or one (the track plays again).
    pub fn set_repeat(&self, mode: RepeatMode) -> Result<(), String> {
        self.send(Command::SetRepeat(mode))
    }

    /// Shuffles the upcoming part, or restores the original order (playing on after the
    /// current track's original place).
    pub fn set_shuffle(&self, on: bool) -> Result<(), String> {
        self.send(Command::SetShuffle(on))
    }

    /// Hands the engine a fresher copy of a track (resolve it off the engine thread). It
    /// replaces the shown track when the ids match and publishes a new state.
    pub fn refresh_track(&self, track: Track) -> Result<(), String> {
        self.send(Command::UpdateTrack(Box::new(track)))
    }

    pub fn toggle(&self) -> Result<(), String> {
        self.send(Command::Toggle)
    }

    /// Pauses when playing (a station is held, as a toggle would); while a track loads it
    /// starts paused. Paused or idle, nothing happens: unlike [`toggle`](Self::toggle) it
    /// never resumes.
    pub fn pause(&self) -> Result<(), String> {
        self.send(Command::Pause)
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
            state: Arc::new(Mutex::new(PlayerState::idle(&options))),
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

    /// See [`PlayerHandle::move_entry`].
    pub fn move_entry(&self, from: usize, to: usize) -> Result<(), String> {
        self.handle.move_entry(from, to)
    }

    /// See [`PlayerHandle::remove`].
    pub fn remove(&self, index: usize) -> Result<(), String> {
        self.handle.remove(index)
    }

    /// See [`PlayerHandle::set_repeat`].
    pub fn set_repeat(&self, mode: RepeatMode) -> Result<(), String> {
        self.handle.set_repeat(mode)
    }

    /// See [`PlayerHandle::set_shuffle`].
    pub fn set_shuffle(&self, on: bool) -> Result<(), String> {
        self.handle.set_shuffle(on)
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
        self.handle.toggle()
    }

    /// See [`PlayerHandle::pause`].
    pub fn pause(&self) -> Result<(), String> {
        self.handle.pause()
    }

    pub fn next(&self) -> Result<(), String> {
        self.handle.next()
    }

    /// See [`PlayerHandle::prev`].
    pub fn prev(&self) -> Result<(), String> {
        self.handle.prev()
    }

    pub fn seek(&self, ms: u64) -> Result<(), String> {
        self.handle.seek(ms)
    }

    /// See [`PlayerHandle::set_volume`].
    pub fn set_volume(&self, volume: f32) -> Result<(), String> {
        self.handle.set_volume(volume)
    }

    /// Moves playback to another output (`None` = the system default), keeping the
    /// position. A device that is gone falls back to the default.
    pub fn set_device(&self, device: Option<String>) -> Result<(), String> {
        self.send(Command::SetDevice(device))
    }

    /// See [`PlayerHandle::refresh_track`].
    pub fn refresh_track(&self, track: Track) -> Result<(), String> {
        self.handle.refresh_track(track)
    }

    /// Off: the next track is opened only once the current one has ended.
    pub fn set_gapless(&self, gapless: bool) -> Result<(), String> {
        self.send(Command::SetGapless(gapless))
    }

    /// Applies at once to the playing track and the pre-opened next one.
    pub fn set_replay_gain(&self, mode: ReplayGain) -> Result<(), String> {
        self.send(Command::SetReplayGain(mode))
    }

    /// The crossfade mode and length (ms), from the next track change on (see
    /// [`crossfade_for`]). Manual next, prev and jump always cut without a fade.
    pub fn set_crossfade(&self, mode: Crossfade, ms: u32) -> Result<(), String> {
        self.send(Command::SetCrossfade(mode, ms))
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
    /// The queue entry (see [`Queue::uid`]): it may move while the job runs.
    uid: u64,
    id: String,
    spec: StreamSpec,
    /// Where the source starts (ms): a resume after an offline pause seeks there on the
    /// opener thread (a network seek can take a request).
    start_ms: u64,
}

/// Why a track could not be opened.
struct OpenFailure {
    message: String,
    /// The server could not be reached ([`is_offline`]): pause on it, do not skip it.
    offline: bool,
    /// Not a failure: a live entry is never pre-opened; it opens after the current one.
    live: bool,
    /// The track, when it was resolved before the open failed.
    track: Option<Box<Track>>,
}

impl OpenFailure {
    fn plain(message: String) -> OpenFailure {
        OpenFailure {
            message,
            offline: false,
            live: false,
            track: None,
        }
    }
}

struct Opened {
    generation: u64,
    slot: Slot,
    uid: u64,
    id: String,
    result: Result<(Track, Box<dyn AudioSource>), OpenFailure>,
}

/// Looks `id` up and checks that it can be played: a local track needs its file, a
/// Navidrome track and a station are streamed.
fn resolve(resolver: &Resolver, id: &str) -> Result<Track, String> {
    match resolver(id) {
        Some(track)
            if track.path.is_some()
                || matches!(track.source, Source::Navidrome | Source::Radio) =>
        {
            Ok(track)
        }
        Some(_) => Err("This track has no file to play.".to_string()),
        None => Err("This track is no longer in the library.".to_string()),
    }
}

/// Resolves and opens `job`'s track, at its start position.
fn open_job(
    resolver: &Resolver,
    opener: &Opener,
    job: &OpenJob,
) -> Result<(Track, Box<dyn AudioSource>), OpenFailure> {
    let track = resolve(resolver, &job.id).map_err(OpenFailure::plain)?;
    if job.slot == Slot::Next && is_live(&track) {
        // A station opens only once it plays: a connection opened early would play stale
        // audio. The engine skips the pre-open by id; this catches any other live entry.
        return Err(OpenFailure {
            message: String::new(),
            offline: false,
            live: true,
            track: Some(Box::new(track)),
        });
    }
    let opened = opener(&track, job.spec).and_then(|mut source| {
        if job.start_ms > 0 {
            source.seek(job.start_ms)?;
        }
        Ok(source)
    });
    match opened {
        Ok(source) => Ok((track, source)),
        Err(err) => Err(OpenFailure {
            message: format!("{err:#}"),
            offline: is_offline(&err),
            live: false,
            track: Some(Box::new(track)),
        }),
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
                let result = catch_unwind(AssertUnwindSafe(|| open_job(&resolver, &opener, &job)))
                    .unwrap_or_else(|_| {
                        Err(OpenFailure::plain("opening the file crashed".to_string()))
                    });
                let opened = Opened {
                    generation: job.generation,
                    slot: job.slot,
                    uid: job.uid,
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

/// The gapless pre-open of the track that follows the current one (the next one, the first
/// one after the last with repeat all, the same one with repeat one). Entries are named by
/// their queue uid, so a move elsewhere in the queue keeps it.
enum Preload {
    None,
    Requested(u64),
    Ready(u64, Box<Track>),
    /// (uid, track id, why): reported when the queue reaches it (an offline one pauses there).
    Failed(u64, String, OpenFailure),
    /// Opened for a crossfade that does not apply (gapless off): dropped, and the track
    /// opens after the current one ends.
    Skipped,
}

/// The player paused on a queue entry because the server could not be reached; Play opens
/// it again at `position_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Held {
    uid: u64,
    position_ms: u64,
    /// Its play was already announced (`TrackChanged`): resuming it is the same play.
    announced: bool,
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
    /// As chosen; see [`Engine::repeat_mode`] for what applies.
    repeat: RepeatMode,
    shuffle: bool,
    sink: EventSink,
    shared: Shared,
    openers: Openers,
    opened: Receiver<Opened>,
    generation: Arc<AtomicU64>,
    status: PlayerStatus,
    volume: f32,
    gapless: bool,
    replay_gain: ReplayGain,
    crossfade: Crossfade,
    crossfade_ms: u32,
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
    /// Paused on an entry the server could not deliver (status `Paused`, nothing decoding).
    held: Option<Held>,
    /// While held: the previous track's tail is still playing out (pumped, never cut).
    draining: bool,
    /// Where the track being loaded starts (ms): 0, or a held position being resumed.
    loading_at: u64,
    /// The track being loaded resumes a play already announced: it starts without a new
    /// `TrackChanged` (which always means a new play).
    loading_announced: bool,
    /// Tracks that failed in a row since one was last heard. Offline failures do not count.
    failures: u32,
    dirty: bool,
    last_emit: Instant,
    /// The last `player://state` sent; an unchanged snapshot is not sent again.
    last_sent: Option<PlayerState>,
    /// When the last [`PlayerEvent::Level`] went out.
    last_level: Instant,
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
        let mut engine = Engine {
            playback,
            queue: Queue::new(),
            queue_version: 0,
            flow: None,
            repeat: options.repeat,
            shuffle: options.shuffle,
            sink,
            shared,
            openers,
            opened,
            generation,
            status: PlayerStatus::Idle,
            volume: options.volume,
            gapless: options.gapless,
            replay_gain: options.replay_gain,
            crossfade: options.crossfade,
            crossfade_ms: options.crossfade_ms,
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
            held: None,
            draining: false,
            loading_at: 0,
            loading_announced: false,
            failures: 0,
            dirty: true,
            last_emit: Instant::now(),
            last_sent: None,
            last_level: Instant::now(),
        };
        engine.crossfade_changed();
        Ok(engine)
    }

    /// The crossfade length that applies to the queue as it is (0 = none); the exception
    /// for consecutive album tracks is made per track change in [`crossfade_for`].
    fn fade_ms(&self) -> u32 {
        match self.crossfade {
            Crossfade::Off => 0,
            Crossfade::Flow if self.flow.is_none() => 0,
            Crossfade::Flow | Crossfade::Always => self.crossfade_ms,
        }
    }

    /// While a crossfade is configured, every source is decoded further ahead (its end must
    /// be decoded with more than the fade still to play) and the next track opens earlier.
    fn crossfade_changed(&mut self) {
        // A next track skipped under the old settings is requested again (once per change;
        // it is skipped again when the fade still does not apply).
        if matches!(self.preload, Preload::Skipped) {
            self.preload = Preload::None;
        }
        let fade_ms = self.fade_ms();
        let target = if fade_ms > 0 {
            READ_AHEAD_MS.max(fade_ms.saturating_add(1_000))
        } else {
            READ_AHEAD_MS
        };
        self.playback.set_read_ahead_ms(target);
        if !self.gapless && fade_ms == 0 {
            // Without gapless and without a fade, nothing is opened early.
            if !matches!(self.preload, Preload::None) {
                self.preload = Preload::None;
                self.playback.queue_next(None);
            }
            return;
        }
        let queued = match (&self.current, &self.preload) {
            (Some(current), Preload::Ready(uid, next)) => {
                Some(self.crossfade_into(current, *uid, next))
            }
            _ => None,
        };
        match queued {
            Some(0) if !self.gapless => self.skip_preloaded(),
            Some(ms) => self.playback.set_next_crossfade(ms),
            None => {}
        }
    }

    /// Gapless is off and the next track does not fade in: it is not queued, so it opens
    /// once the current one has ended (a real gap), and it is not requested again.
    fn skip_preloaded(&mut self) {
        self.playback.queue_next(None);
        self.preload = Preload::Skipped;
    }

    /// How long `next` blends into `current`.
    fn crossfade_to(&self, current: &Track, next: &Track) -> u32 {
        crossfade_for(
            self.crossfade,
            self.crossfade_ms,
            self.flow.is_some(),
            current,
            next,
        )
    }

    /// How long the queue entry `uid` (`next`) blends into `current`. Repeat one hands the
    /// track over to itself without a fade, like a gapless loop.
    fn crossfade_into(&self, current: &Track, uid: u64, next: &Track) -> u32 {
        if self.queue.uid(self.queue.index()) == Some(uid) {
            0
        } else {
            self.crossfade_to(current, next)
        }
    }

    /// The repeat mode that applies: a Flow never ends and never repeats.
    fn repeat_mode(&self) -> RepeatMode {
        if self.flow.is_some() {
            RepeatMode::Off
        } else {
            self.repeat
        }
    }

    /// The entry that follows the current one. `ended`: the current track played to its
    /// end (repeat one plays it again); otherwise it failed and the queue moves on.
    fn follow_index(&self, ended: bool) -> Option<usize> {
        match self.repeat_mode() {
            RepeatMode::One if ended && !self.queue.is_empty() => Some(self.queue.index()),
            RepeatMode::All => self.queue.wrap_next(),
            RepeatMode::Off | RepeatMode::One => self.queue.next_index(),
        }
    }

    /// The queue (or the repeat mode) changed: a pre-opened track that no longer follows
    /// the current one is dropped, so the right one is opened in its place.
    fn follow_changed(&mut self) {
        let follows = self
            .follow_index(true)
            .and_then(|index| self.queue.uid(index));
        let keep = match &self.preload {
            Preload::None => true,
            Preload::Requested(uid) | Preload::Ready(uid, _) | Preload::Failed(uid, _, _) => {
                Some(*uid) == follows
            }
            // Decided again for whatever follows now.
            Preload::Skipped => false,
        };
        if keep {
            return;
        }
        if matches!(self.preload, Preload::Ready(..)) {
            self.playback.queue_next(None);
        }
        self.preload = Preload::None;
    }

    /// Runs `change` on the queue, keeping the shown index on the shown entry; publishes the
    /// new ids and re-decides the pre-opened track.
    fn change_queue(&mut self, change: impl FnOnce(&mut Queue) -> bool) -> bool {
        let shown = self.queue.uid(self.shown_index);
        if !change(&mut self.queue) {
            return false;
        }
        if let Some(index) = shown.and_then(|uid| self.queue.position_of(uid)) {
            self.shown_index = index;
        }
        self.queue_changed();
        self.follow_changed();
        true
    }

    /// How long before the current track's end the next one is opened.
    fn preload_ms(&self) -> u64 {
        match self.fade_ms() {
            0 => PRELOAD_MS,
            ms => PRELOAD_MS.max(ms as u64 + 3_000),
        }
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
            self.send_level();
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
            || self.draining
    }

    fn handle(&mut self, command: Command) {
        // Nothing to skip while idle (play/toggle start the queue); an ignored command
        // changes nothing and emits nothing. A station has nothing to skip to either (spec
        // 5.9), whether the UI, a media key or the tray asks.
        if matches!(command, Command::Next | Command::Prev)
            && (self.status == PlayerStatus::Idle || self.live_now())
        {
            return;
        }
        self.dirty = true;
        match command {
            Command::Play(ids, start, flow) => {
                // A pending hand-over is settled at the index it belongs to.
                self.settle_handover();
                self.flow = flow;
                self.crossfade_changed();
                // A Flow deals its own order.
                let shuffle = self.shuffle && self.flow.is_none();
                self.queue.set(ids, start, shuffle);
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
            Command::Pause => self.pause(),
            Command::Next => {
                self.settle_handover();
                // Repeat one does not hold a manual next; repeat all goes round.
                let next = match self.repeat_mode() {
                    RepeatMode::All => self.queue.wrap_next(),
                    RepeatMode::Off | RepeatMode::One => self.queue.next_index(),
                };
                match next {
                    Some(index) => {
                        self.queue.jump(index);
                        self.start_current(true);
                    }
                    None => self.go_idle(),
                }
            }
            Command::SetRepeat(mode) => {
                self.repeat = mode;
                self.follow_changed();
            }
            Command::SetShuffle(on) => {
                self.shuffle = on;
                if self.flow.is_none() {
                    self.change_queue(|queue| {
                        let changed = queue.is_shuffled() != on;
                        queue.set_shuffled(on);
                        changed
                    });
                }
            }
            Command::Move(from, to) => {
                self.change_queue(|queue| queue.move_entry(from, to));
            }
            Command::Remove(index) => self.remove(index),
            Command::Prev => self.prev(),
            Command::Seek(ms) => self.seek(ms),
            Command::SetVolume(volume) => {
                self.volume = volume.clamp(0.0, 1.0);
                self.playback.set_volume(self.volume);
            }
            Command::SetDevice(device) => {
                if let Err(err) = self.playback.set_device(device) {
                    // Held offline nothing decodes, so nothing failed: stay held.
                    if self.held.is_none() {
                        self.fail_current(err);
                    }
                }
            }
            Command::SetGapless(gapless) => {
                self.gapless = gapless;
                // A crossfade still needs the track opened early; without one, a track
                // opened early is dropped and opens again once this one ends. Turned on, a
                // skipped next track is opened again for a gapless hand-over.
                self.crossfade_changed();
            }
            Command::UpdateTrack(track) => {
                let track = *track;
                // Not `TrackChanged`: that counts a play. The state event carries the track to
                // the UI and the media controls.
                if self
                    .shown
                    .as_ref()
                    .is_some_and(|shown| shown.id == track.id)
                {
                    if self.current.as_ref().is_some_and(|c| c.id == track.id) {
                        self.current = Some(track.clone());
                    }
                    self.shown = Some(track);
                    self.dirty = true;
                }
            }
            Command::SetReplayGain(mode) => {
                self.replay_gain = mode;
                if let Some(track) = &self.current {
                    // Playing already: a ramp, not a jump.
                    self.playback.ramp_gain(track_gain(track, mode));
                }
                if let Preload::Ready(_, track) = &self.preload {
                    self.playback.set_next_gain(track_gain(track, mode));
                }
            }
            Command::SetCrossfade(mode, ms) => {
                self.crossfade = mode;
                self.crossfade_ms = ms;
                self.crossfade_changed();
            }
            Command::Shutdown => {}
        }
    }

    /// The queue's ids changed: bump the version and publish the ids for handle readers.
    fn queue_changed(&mut self) {
        self.queue_version += 1;
        write_lock(&self.shared.queue, Arc::new(self.queue.ids()));
        self.dirty = true;
    }

    /// Adds `ids` at the end. Playback goes on; when only a tail was left, the first appended
    /// track follows it (even when the queue gave up on failed tracks before the end).
    fn append(&mut self, ids: Vec<String>) {
        if ids.is_empty() {
            return;
        }
        let first = self.queue.len();
        // The next track may be another one now (shuffled in, or after the last with repeat
        // all): decide for it again.
        self.change_queue(|queue| {
            queue.extend(ids);
            true
        });
        if self.ending && self.queue.jump(first) {
            self.failures = 0;
            self.start_current(false);
        }
    }

    /// Removes the queue entry at `index`. When it is the track that plays (or loads), the
    /// one that followed it plays; after the last one, the first with repeat all, else idle.
    /// Paused stays paused, on the track that follows.
    ///
    /// During a gapless hand-over the queue already stands on the next track while the UI
    /// still shows the previous one. Removing the shown one settles the hand-over first (the
    /// next track is what plays anyway). Removing the next one drops it unheard: the shown
    /// track's tail plays out and the entry after it follows.
    fn remove(&mut self, index: usize) {
        if index >= self.queue.len() {
            return;
        }
        let mut cut = true;
        if self.handover.is_some() {
            if index == self.queue.index() {
                self.handover = None;
                self.decoding = false;
                self.current = None;
                if self.status == PlayerStatus::Paused {
                    // Nothing is heard while paused: nothing is cut.
                    self.playback.stop();
                } else {
                    self.playback.drop_sources();
                    cut = false;
                }
            } else if index == self.shown_index {
                self.settle_handover();
            }
        }
        let paused = match self.status {
            PlayerStatus::Paused => true,
            PlayerStatus::Loading => self.start_paused,
            PlayerStatus::Playing | PlayerStatus::Idle => false,
        };
        let current = index == self.queue.index();
        if !self.change_queue(|queue| queue.remove(index)) {
            return;
        }
        if self.queue.is_empty() {
            self.go_idle();
            return;
        }
        if !current || self.status == PlayerStatus::Idle {
            return;
        }
        self.failures = 0;
        if index < self.queue.len() {
            self.start_current(cut);
        } else if self.repeat_mode() == RepeatMode::All {
            self.queue.jump(0);
            self.start_current(cut);
        } else {
            self.finish(cut);
            return;
        }
        if self.status == PlayerStatus::Loading {
            self.start_paused = paused;
        }
    }

    /// Only what is heard (or about to be) is paused; see [`PlayerHandle::pause`].
    fn pause(&mut self) {
        match self.status {
            PlayerStatus::Playing => self.pause_playing(),
            PlayerStatus::Loading => self.start_paused = true,
            PlayerStatus::Paused | PlayerStatus::Idle => {}
        }
    }

    fn pause_playing(&mut self) {
        if self.current.as_ref().is_some_and(is_live) {
            self.pause_live();
        } else {
            self.playback.set_paused(true);
            self.status = PlayerStatus::Paused;
            self.paused_at = Some(Instant::now());
        }
    }

    fn toggle(&mut self) {
        match self.status {
            PlayerStatus::Playing => self.pause_playing(),
            PlayerStatus::Paused if self.held.is_some() => self.resume_held(),
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

    /// Opens the held entry again at the held position (the server may be back). Offline
    /// again, it is held again at the same position, with one more notice.
    fn resume_held(&mut self) {
        let Some(held) = self.held.take() else {
            return;
        };
        if let Some(index) = self.queue.position_of(held.uid) {
            self.queue.jump(index);
        }
        self.start_current_at(true, held.position_ms);
        if self.status == PlayerStatus::Loading {
            self.loading_announced = held.announced;
        }
    }

    /// The shown track's duration, clamping `ms` (0 = unknown, no clamp).
    fn clamp_to_shown(&self, ms: u64) -> u64 {
        match self.shown.as_ref().map_or(0, |track| track.duration_ms) {
            0 => ms,
            duration => ms.min(duration),
        }
    }

    /// True while the current track can be seeked (a station never can).
    fn seekable(&self) -> bool {
        self.decoding
            && matches!(self.status, PlayerStatus::Playing | PlayerStatus::Paused)
            && !self.current.as_ref().is_some_and(is_live)
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
        } else if self.held.is_some() && self.position_ms() > RESTART_AFTER_MS {
            // Held offline: restart the held track (it stays paused).
            self.seek(0);
        } else if self.seekable() && self.position_ms() > RESTART_AFTER_MS {
            self.seek(0);
        } else if self.queue.back() {
            self.start_current(true);
        } else if self.repeat_mode() == RepeatMode::All && self.queue.len() > 1 {
            // Repeat all goes round backwards too.
            self.queue.jump(self.queue.len() - 1);
            self.start_current(true);
        } else if self.seekable() || self.held.is_some() {
            self.seek(0);
        }
    }

    /// A seek that changes nothing (not seekable, or to where it already is) is ignored.
    /// Seeks are clamped to the track's duration. Held offline, a seek moves the position
    /// Play resumes at.
    fn seek(&mut self, ms: u64) {
        if self.live_now() || self.current.as_ref().is_some_and(is_live) {
            return;
        }
        if self.held.is_some() {
            let ms = self.clamp_to_shown(ms);
            if let Some(held) = self.held.as_mut() {
                held.position_ms = ms;
            }
            return;
        }
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
        self.start_current_at(cut, 0);
    }

    /// Like [`start_current`](Self::start_current), starting at `start_ms`.
    fn start_current_at(&mut self, cut: bool, start_ms: u64) {
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
        self.held = None;
        // A tail still playing goes on (it is pumped while loading) unless it was cut.
        self.draining = false;
        self.loading_at = start_ms;
        self.loading_announced = false;
        self.dirty = true;
        let (Some(id), Some(uid)) = (
            self.queue.current().map(str::to_string),
            self.queue.uid(self.queue.index()),
        ) else {
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
            uid,
            id: id.clone(),
            spec: self.playback.output_spec(),
            start_ms,
        };
        if let Err(message) = self.openers.send(job) {
            self.error(&id, source_of(None, &id), &message);
            // Every other track would fail the same way.
            self.finish(cut);
        }
    }

    /// Moves past a track that failed: the next one (round to the first with repeat all;
    /// repeat one does not retry it), or the end of the queue. With repeat all, a queue of
    /// one entry tries that entry again. After [`MAX_FAILURES_IN_A_ROW`] failures in a row
    /// the player gives up and goes idle, so such a retry stops after 10 attempts. A station
    /// (`live`) is not retried that way: repeat has no effect on it.
    fn skip(&mut self, cut: bool, live: bool) {
        if live && self.follow_index(false) == Some(self.queue.index()) {
            self.finish(cut);
            return;
        }
        self.play_on(false, cut);
    }

    /// Moves on from the current track: to the one that follows it (see
    /// [`follow_index`](Self::follow_index)), or to the end of the queue.
    fn play_on(&mut self, ended: bool, cut: bool) {
        if self.failures >= MAX_FAILURES_IN_A_ROW {
            self.finish(cut);
            return;
        }
        match self.follow_index(ended) {
            Some(index) => {
                self.queue.jump(index);
                self.start_current(cut);
            }
            None => self.finish(cut),
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
        self.held = None;
        self.draining = false;
        self.loading_at = 0;
        self.loading_announced = false;
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
                if self.queue.uid(self.queue.index()) != Some(opened.uid)
                    || self.status != PlayerStatus::Loading
                {
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
                    Err(failure) if failure.offline => {
                        let (at, announced) = (self.loading_at, self.loading_announced);
                        let track = failure.track.map(|track| *track);
                        self.hold(track, &opened.id, at, announced);
                    }
                    Err(failure) => {
                        let track = failure.track.as_deref();
                        // An unresolvable station is still a station.
                        let live = track.map_or(opened.id.starts_with(RADIO_PREFIX), is_live);
                        let source = source_of(track, &opened.id);
                        self.error(&opened.id, source, &failure.message);
                        self.skip(false, live);
                    }
                }
            }
            Slot::Next => {
                if !matches!(self.preload, Preload::Requested(uid) if uid == opened.uid) {
                    return;
                }
                self.preload = match opened.result {
                    Ok((track, source)) => {
                        let crossfade_ms = self.current.as_ref().map_or(0, |current| {
                            self.crossfade_into(current, opened.uid, &track)
                        });
                        if crossfade_ms == 0 && !self.gapless {
                            // Opened only for a fade that does not apply to this change.
                            self.skip_preloaded();
                            return;
                        }
                        self.playback.queue_next_with(
                            Some(source),
                            NextOptions {
                                gain: track_gain(&track, self.replay_gain),
                                crossfade_ms,
                            },
                        );
                        Preload::Ready(opened.uid, Box::new(track))
                    }
                    Err(failure) if failure.live => {
                        self.skip_preloaded();
                        return;
                    }
                    Err(failure) => Preload::Failed(opened.uid, opened.id, failure),
                };
            }
        }
    }

    fn tick(&mut self) {
        // While paused nothing is heard, so a pending hand-over waits. So does one whose
        // source already failed: the pump reports it first, as a hand-over never heard.
        if self.status != PlayerStatus::Paused && !self.playback.has_deferred_error() {
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
        if matches!(self.status, PlayerStatus::Playing | PlayerStatus::Loading) || self.draining {
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
        if self.draining && self.playback.is_drained() {
            self.draining = false;
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
        if self.start_paused && self.current.as_ref().is_some_and(is_live) {
            // Paused while it loaded: a station is not kept open while paused.
            drop(source);
            let track = self.current.clone();
            let announced = std::mem::take(&mut self.loading_announced);
            self.hold_entry(track, 0, announced);
            return;
        }
        // A resumed track was seeked on the opener thread already.
        let start_ms = std::mem::take(&mut self.loading_at);
        match self.playback.set_source_at(source, start_ms) {
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
                    if std::mem::take(&mut self.loading_announced) {
                        // The same play, resumed after an offline hold: not a new one.
                        self.shown = Some(track);
                        self.shown_index = self.queue.index();
                        self.failures = 0;
                        self.dirty = true;
                    } else {
                        self.show(track);
                    }
                }
            }
            Err(err) => {
                let id = self.current_id();
                let source = source_of(self.current.as_ref(), &id);
                let live = self.current.as_ref().is_some_and(is_live);
                self.error(&id, source, &format!("{err:#}"));
                self.skip(false, live);
            }
        }
    }

    /// Opens the next track once the current one is within [`PRELOAD_MS`] of its end (with a
    /// crossfade, `max(PRELOAD_MS, crossfade + 3 s)`), or once it has been decoded to its
    /// end. The second rule needs no duration, so tracks whose length the library does not
    /// know (often FFmpeg-decoded ones) are gapless too. With gapless off, only a crossfade
    /// opens the next track early.
    fn maybe_preload(&mut self) {
        if (!self.gapless && self.fade_ms() == 0)
            || !matches!(self.preload, Preload::None)
            || !self.seekable()
        {
            return;
        }
        let Some(track) = &self.current else {
            return;
        };
        let Some(index) = self.follow_index(true) else {
            return;
        };
        // Durations come from the library (FFmpeg sources do not know theirs).
        let near_end = track.duration_ms > 0
            && track
                .duration_ms
                .saturating_sub(self.playback.position_ms())
                <= self.preload_ms();
        if !near_end && !self.playback.current_exhausted() {
            return;
        }
        // A source that failed reads as decoded to its end, but nothing plays on after it:
        // its error decides (an offline one holds the queue there).
        if self.playback.current_failed() {
            return;
        }
        let (Some(id), Some(uid)) = (
            self.queue.get(index).map(str::to_string),
            self.queue.uid(index),
        ) else {
            return;
        };
        // A station is never opened early (and so never blended in): it opens once the
        // current track has ended. The id is enough; the engine thread does not resolve.
        if id.starts_with(RADIO_PREFIX) {
            return;
        }
        let job = OpenJob {
            generation: self.generation.load(Ordering::SeqCst),
            slot: Slot::Next,
            uid,
            id: id.clone(),
            spec: self.playback.output_spec(),
            start_ms: 0,
        };
        self.preload = match self.openers.send(job) {
            Ok(()) => Preload::Requested(uid),
            Err(message) => Preload::Failed(uid, id, OpenFailure::plain(message)),
        };
    }

    /// The pre-opened next track took over; the UI hears about it once it is audible.
    fn on_advanced(&mut self) {
        let Preload::Ready(uid, track) = std::mem::replace(&mut self.preload, Preload::None) else {
            return;
        };
        self.settle_handover();
        if let Some(index) = self.queue.position_of(uid) {
            self.queue.jump(index);
        }
        self.current = Some((*track).clone());
        self.decoding = true;
        let audible_at = Instant::now() + Duration::from_millis(self.playback.queued_ms());
        self.handover = Some((audible_at, *track));
    }

    /// The current track ended with nothing queued behind it.
    fn on_finished(&mut self) {
        self.decoding = false;
        if let Some(track) = self.current.clone().filter(is_live) {
            // A station never ends: one that closed its stream is as good as unreachable,
            // so the player holds it instead of going idle.
            let id = track.id.clone();
            self.hold(Some(track), &id, 0, true);
            return;
        }
        let follows = self.follow_index(true);
        match std::mem::replace(&mut self.preload, Preload::None) {
            Preload::Failed(uid, id, failure)
                if !failure.offline
                    && follows.and_then(|index| self.queue.uid(index)) == Some(uid) =>
            {
                if let Some(index) = follows {
                    self.queue.jump(index);
                }
                let source = source_of(failure.track.as_deref(), &id);
                self.error(&id, source, &failure.message);
                self.skip(false, false);
            }
            // A pre-open that failed offline is tried once more now, as the track to play
            // (the server may be back): offline again, the player holds there.
            _ => self.play_on(true, false),
        }
    }

    /// A decode or seek error: report it and move on. An offline one pauses on the track
    /// instead, at the position it reached.
    fn fail_current(&mut self, err: anyhow::Error) {
        let id = self.current_id();
        // A hand-over to the failing track was never heard: the previous track's tail is
        // still playing and is not cut.
        let unheard = self.handover.take();
        if is_offline(&err) {
            let (track, position, announced) = match unheard {
                // Never heard, never announced.
                Some((_, track)) => (Some(track), 0, false),
                None => {
                    let position = self.clamp_to_current(self.playback.position_ms());
                    // Everything up to the position was heard; nothing after it is.
                    self.playback.stop();
                    // Decoding without a pending hand-over: it was started (or settled) and
                    // announced.
                    (self.current.clone(), position, true)
                }
            };
            self.hold(track, &id, position, announced);
            return;
        }
        let unheard = unheard.is_some();
        let source = source_of(self.current.as_ref(), &id);
        let live = self.current.as_ref().is_some_and(is_live);
        self.error(&id, source, &format!("{err:#}"));
        self.decoding = false;
        if unheard {
            self.playback.drop_sources();
        } else {
            self.playback.stop();
        }
        self.skip(!unheard, live);
    }

    /// `ms` clamped to the current track's duration (when known).
    fn clamp_to_current(&self, ms: u64) -> u64 {
        match self.current.as_ref().map_or(0, |track| track.duration_ms) {
            0 => ms,
            duration => ms.min(duration),
        }
    }

    /// The server could not deliver the queue's current entry (`track`, when resolved): stay
    /// on it, paused at `position_ms`, with one [`ErrorKind::Offline`] notice. Nothing is
    /// skipped and the failure cap is not touched; Play opens it again at the position. A
    /// previous track's tail that is still in the output plays out.
    ///
    /// `announced`: the track's play was already announced (`TrackChanged`), so resuming it
    /// does not announce it again.
    fn hold(&mut self, track: Option<Track>, track_id: &str, position_ms: u64, announced: bool) {
        let source = source_of(track.as_ref(), track_id);
        self.hold_entry(track, position_ms, announced);
        let message = match source {
            Source::Radio => STATION_OFFLINE_MESSAGE,
            Source::Local | Source::Navidrome => OFFLINE_MESSAGE,
        };
        (self.sink)(PlayerEvent::Error(PlayerError {
            track_id: track_id.to_string(),
            message: message.to_string(),
            kind: ErrorKind::Offline,
            source,
        }));
    }

    /// [`hold`](Self::hold) without the notice (a paused station). A station is always held
    /// at 0: Play joins it live.
    fn hold_entry(&mut self, track: Option<Track>, position_ms: u64, announced: bool) {
        // A pre-open still running for the next entry is not wanted any more.
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.playback.drop_sources();
        self.draining = !self.playback.is_drained();
        self.current = None;
        self.decoding = false;
        self.ready = None;
        self.preload = Preload::None;
        self.handover = None;
        self.ending = false;
        self.start_paused = false;
        self.paused_at = None;
        self.loading_at = 0;
        self.loading_announced = false;
        let live = track.as_ref().is_some_and(is_live);
        if let Some(track) = track {
            // Shown, but not `TrackChanged`: nothing of it was heard (or it was already).
            self.shown = Some(track);
        }
        self.shown_index = self.queue.index();
        let position_ms = if live {
            0
        } else {
            self.clamp_to_shown(position_ms)
        };
        self.held = self.queue.uid(self.queue.index()).map(|uid| Held {
            uid,
            position_ms,
            announced,
        });
        self.status = PlayerStatus::Paused;
        self.dirty = true;
    }

    /// Pausing a station closes its stream (and connection); Play joins it live again, as
    /// the same play.
    fn pause_live(&mut self) {
        let track = self.current.clone();
        // Nothing of it is kept to play later, so nothing waits in the output either.
        self.playback.stop();
        self.hold_entry(track, 0, true);
    }

    /// The queue stands on a station (playing, loading or held): next, prev and seek do
    /// nothing. Decided by the entry's id, so a jump away from a station acts at once.
    fn live_now(&self) -> bool {
        self.queue
            .current()
            .is_some_and(|id| id.starts_with(RADIO_PREFIX))
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
    fn error(&mut self, track_id: &str, source: Source, message: &str) {
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
            kind: ErrorKind::Track,
            source,
        }));
    }

    fn position_ms(&self) -> u64 {
        if let Some(held) = &self.held {
            return held.position_ms;
        }
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
            repeat: self.repeat,
            shuffle: self.shuffle,
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

    /// Every [`LEVEL_INTERVAL`] while playing: the loudness for the ring's glow. Kept apart
    /// from [`publish`](Self::publish): it changes neither the state nor its dedupe.
    fn send_level(&mut self) {
        if self.status != PlayerStatus::Playing || self.last_level.elapsed() < LEVEL_INTERVAL {
            return;
        }
        self.last_level = Instant::now();
        (self.sink)(PlayerEvent::Level(self.playback.level()));
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
    use crate::audio::{open_source, Input, Offline, OutputEvent, OutputSink};
    use crate::db::Db;
    use crate::library::history::spawn_recorder;
    use crate::library::tags::{self, fixtures};
    use crate::navidrome::scrobble::{ScrobbleAction, ScrobbleTracker};
    use crate::player::dropped::tests::drop_files;
    use crate::player::dropped::{with_dropped, DroppedFiles};
    use crate::player::fan_out;
    use crate::radio::model::tests::station as test_station;
    use crate::radio::model::RADIO_PREFIX;

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
    fn refresh_track_replaces_the_shown_track_without_a_new_play() {
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
        let h = harness(tracks.clone(), 1.0, Some(opener));
        h.player.play(ids(&["long1", "long2"]), 0).unwrap();
        collect_until(&h.events, Duration::from_secs(5), |seen| {
            changes(seen) == ids(&["long1"])
        });

        let mut other = tracks[1].clone();
        other.cover_id = Some("other".into());
        h.player.refresh_track(other).unwrap();
        let mut fresh = tracks[0].clone();
        fresh.cover_id = Some("fresh".into());
        h.player.refresh_track(fresh).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(5), |seen| {
            seen.iter().any(|event| {
                matches!(event, PlayerEvent::State(state)
                    if state.track.as_ref().is_some_and(|t| t.cover_id.as_deref() == Some("fresh")))
            })
        });
        assert!(changes(&events).is_empty(), "a refresh is not a new play");
        let state = h.player.state();
        let shown = state.track.expect("still shown");
        assert_eq!(shown.id, "long1");
        assert_eq!(state.queue_index, 0);
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
            repeat: RepeatMode::One,
            shuffle: true,
        };
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(value["repeat"], "one");
        assert_eq!(value["shuffle"], true);
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
            kind: ErrorKind::Track,
            source: Source::Local,
        })
        .unwrap();
        assert_eq!(error["trackId"], "t");
        assert_eq!(error["kind"], "track");
        assert_eq!(error["source"], "local");
        let offline = PlayerError {
            track_id: STATION.into(),
            message: STATION_OFFLINE_MESSAGE.into(),
            kind: ErrorKind::Offline,
            source: Source::Radio,
        };
        let offline = serde_json::to_value(offline).unwrap();
        assert_eq!(offline["kind"], "offline");
        assert_eq!(offline["source"], "radio");
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

    /// A track of `album` at `track_no`.
    fn album_track(id: &str, album: &str, track_no: u32) -> Track {
        let mut track = fixture_track(id, "tagged.flac");
        track.album_id = album.to_string();
        track.track_no = Some(track_no);
        track
    }

    #[test]
    fn crossfade_for_rules() {
        let other = album_track("o", "y", 2);
        let on_disc = |id: &str, disc: u32, no: u32| {
            let mut track = album_track(id, "x", no);
            track.disc_no = Some(disc);
            track
        };
        let one = on_disc("1", 1, 1);
        let two = on_disc("2", 1, 2);
        let three = on_disc("3", 1, 3);
        let mut untracked = on_disc("u", 1, 2);
        untracked.track_no = None;
        let last_of_disc_1 = on_disc("d1", 1, 12);
        let first_of_disc_2 = on_disc("d2", 2, 1);
        let first_of_disc_3 = on_disc("d3", 3, 1);
        let two_on_disc_2 = on_disc("2b", 2, 2);
        let no_album_1 = album_track("n1", "", 1);
        let no_album_2 = album_track("n2", "", 2);
        let cases = [
            // (mode, in Flow, current, next, expected)
            (Crossfade::Off, false, &one, &other, 0),
            (Crossfade::Off, true, &one, &other, 0),
            (Crossfade::Flow, false, &one, &other, 0),
            (Crossfade::Flow, true, &one, &other, 4_000),
            (Crossfade::Flow, true, &one, &two, 4_000),
            (Crossfade::Always, false, &one, &other, 4_000),
            (Crossfade::Always, true, &one, &other, 4_000),
            // The following track of the same album stays gapless, in Flow or not.
            (Crossfade::Always, false, &one, &two, 0),
            (Crossfade::Always, true, &two, &three, 0),
            // Not the following one: a skip within the album, a step back, no number.
            (Crossfade::Always, false, &one, &three, 4_000),
            (Crossfade::Always, false, &two, &one, 4_000),
            (Crossfade::Always, false, &one, &untracked, 4_000),
            // Discs: track 1 of the next disc goes on; the same number on another disc not.
            (
                Crossfade::Always,
                false,
                &last_of_disc_1,
                &first_of_disc_2,
                0,
            ),
            (Crossfade::Always, false, &one, &two_on_disc_2, 4_000),
            (
                Crossfade::Always,
                false,
                &last_of_disc_1,
                &first_of_disc_3,
                4_000,
            ),
            // An unknown album is never one album.
            (Crossfade::Always, false, &no_album_1, &no_album_2, 4_000),
        ];
        for (mode, in_flow, current, next, expected) in cases {
            assert_eq!(
                crossfade_for(mode, 4_000, in_flow, current, next),
                expected,
                "{mode:?}, in Flow {in_flow}, {} -> {}",
                current.id,
                next.id
            );
        }
    }

    /// An opener of [`Tone`]s: `values` maps a track id to its value and length.
    fn tone_opener(values: Vec<(&'static str, f32, u64)>) -> Opener {
        Arc::new(move |track: &Track, _spec: StreamSpec| {
            let (_, value, ms) = values
                .iter()
                .find(|(id, _, _)| *id == track.id)
                .copied()
                .ok_or_else(|| anyhow::anyhow!("no tone for {}", track.id))?;
            Ok(Box::new(Tone::new(value, ms)) as Box<dyn AudioSource>)
        })
    }

    fn crossfade_always(ms: u32) -> PlayerOptions {
        PlayerOptions {
            crossfade: Crossfade::Always,
            crossfade_ms: ms,
            ..PlayerOptions::default()
        }
    }

    #[test]
    fn crossfade_blends_the_next_track_in() {
        assert_blends(crossfade_always(1_000));
    }

    #[test]
    fn crossfade_blends_with_gapless_off() {
        assert_blends(PlayerOptions {
            gapless: false,
            ..crossfade_always(1_000)
        });
    }

    /// Plays 2 s tones "a" (album x, track 1) then "c" (album `c_album`, track 2) with
    /// `options` at 5x; returns the events and what was played.
    fn play_two_tones(options: PlayerOptions, c_album: &str) -> (Vec<PlayerEvent>, Vec<f32>) {
        let mut a = album_track("a", "x", 1);
        a.duration_ms = 2_000;
        let mut c = album_track("c", c_album, 2);
        c.duration_ms = 2_000;
        let tracks = [a, c];
        let resolver: Resolver =
            Arc::new(move |id: &str| tracks.iter().find(|track| track.id == id).cloned());
        let opener = tone_opener(vec![("a", TONE_A, 2_000), ("c", TONE_C, 2_000)]);
        let h = harness_full(resolver, 5.0, Some(opener), options);
        h.player.play(ids(&["a", "c"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), back_to_idle);
        let played = h.played.lock().unwrap().clone();
        (events, played)
    }

    #[test]
    fn same_album_with_gapless_off_keeps_the_gap() {
        let (events, played) = play_two_tones(
            PlayerOptions {
                gapless: false,
                ..crossfade_always(1_000)
            },
            "x",
        );
        assert_eq!(changes(&events), ids(&["a", "c"]));
        let first = events
            .iter()
            .position(|e| matches!(e, PlayerEvent::TrackChanged(_)))
            .unwrap();
        assert!(
            events[first..]
                .iter()
                .any(|e| is_state(e, PlayerStatus::Loading)),
            "c was loaded after a ended: {events:#?}"
        );
        assert_eq!(
            played.len(),
            2 * 2 * 88_200,
            "both tones whole, nothing blended"
        );
        assert!(played
            .iter()
            .all(|s| (*s - TONE_A).abs() < 1e-6 || (*s - TONE_C).abs() < 1e-6));
    }

    #[test]
    fn gapless_on_after_a_skipped_preload_hands_over_without_a_gap() {
        let mut a = album_track("a", "x", 1);
        a.duration_ms = 2_000;
        let mut c = album_track("c", "x", 2);
        c.duration_ms = 2_000;
        let tracks = [a, c];
        let resolver: Resolver =
            Arc::new(move |id: &str| tracks.iter().find(|track| track.id == id).cloned());
        let (preloaded_tx, preloaded) = crossbeam_channel::unbounded();
        let tones = tone_opener(vec![("a", TONE_A, 2_000), ("c", TONE_C, 2_000)]);
        let opener: Opener = Arc::new(move |track: &Track, spec: StreamSpec| {
            let source = tones(track, spec);
            if std::thread::current().name() == Some(OPENER_NEXT_THREAD) {
                let _ = preloaded_tx.send(());
            }
            source
        });
        let options = PlayerOptions {
            gapless: false,
            ..crossfade_always(1_000)
        };
        let h = harness_full(resolver, 2.0, Some(opener), options);
        h.player.play(ids(&["a", "c"]), 0).unwrap();
        // The fade opens "c" early; the same-album change does not fade, so it is skipped.
        preloaded
            .recv_timeout(Duration::from_secs(5))
            .expect("c is pre-opened for the fade");
        std::thread::sleep(Duration::from_millis(50));
        h.player.set_gapless(true).unwrap();
        preloaded
            .recv_timeout(Duration::from_secs(5))
            .expect("c is opened again for the gapless hand-over");
        let events = collect_until(&h.events, Duration::from_secs(10), back_to_idle);
        assert_eq!(changes(&events), ids(&["a", "c"]));
        let first = events
            .iter()
            .position(|e| matches!(e, PlayerEvent::TrackChanged(_)))
            .unwrap();
        assert!(
            !events[first..]
                .iter()
                .any(|e| is_state(e, PlayerStatus::Loading)),
            "c was handed over gaplessly, not loaded after a ended: {events:#?}"
        );
        let played = h.played.lock().unwrap().clone();
        assert_eq!(played.len(), 2 * 2 * 88_200, "both tones whole");
        assert!(played
            .iter()
            .all(|s| (*s - TONE_A).abs() < 1e-6 || (*s - TONE_C).abs() < 1e-6));
    }

    fn assert_blends(options: PlayerOptions) {
        let (events, played) = play_two_tones(options, "y");
        assert_eq!(changes(&events), ids(&["a", "c"]));
        let frames = played.len() / 2;
        assert!(
            frames.abs_diff(3 * 44_100) <= 4_096,
            "{frames} frames for 2 s + 2 s with a 1 s overlap"
        );
        let blended = played
            .iter()
            .filter(|s| (**s - TONE_A).abs() > 1e-6 && (**s - TONE_C).abs() > 1e-6)
            .count();
        assert!(blended > 2 * 35_000, "{blended} samples of the two mixed");
        assert!(
            played.iter().all(|s| *s <= 0.56),
            "equal power, no overshoot"
        );
    }

    #[test]
    fn manual_next_does_not_fade() {
        let mut a = album_track("a", "x", 1);
        a.duration_ms = SILENCE_MS;
        let mut b = album_track("b", "y", 1);
        b.duration_ms = 1_000;
        let tracks = [a, b];
        let resolver: Resolver =
            Arc::new(move |id: &str| tracks.iter().find(|track| track.id == id).cloned());
        let (preloaded_tx, preloaded) = crossbeam_channel::unbounded();
        let tones = tone_opener(vec![("a", TONE_A, SILENCE_MS), ("b", TONE_C, 1_000)]);
        let opener: Opener = Arc::new(move |track: &Track, spec: StreamSpec| {
            let source = tones(track, spec);
            if std::thread::current().name() == Some(OPENER_NEXT_THREAD) {
                let _ = preloaded_tx.send(());
            }
            source
        });
        let h = harness_full(resolver, 1.0, Some(opener), crossfade_always(4_000));
        h.player.play(ids(&["a", "b"]), 0).unwrap();
        // With a 4 s fade the next track opens 7 s ahead: at once for "b".
        preloaded
            .recv_timeout(Duration::from_secs(5))
            .expect("b is pre-opened with a fade queued");
        std::thread::sleep(Duration::from_millis(50));
        h.player.next().unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), back_to_idle);
        assert_eq!(changes(&events), ids(&["a", "b"]));
        let played = h.played.lock().unwrap().clone();
        assert!(
            played
                .iter()
                .all(|s| (*s - TONE_A).abs() < 1e-6 || (*s - TONE_C).abs() < 1e-6),
            "a was cut, nothing blended"
        );
        assert_eq!(
            played_of(&h.played, TONE_C),
            44_100 * 2,
            "all of b, from its start"
        );
        assert!(played_of(&h.played, TONE_A) < TONE_A_SAMPLES * 4);
    }

    /// An idle state arrived (for a queue that was already playing).
    fn ends_idle(events: &[PlayerEvent]) -> bool {
        events.iter().any(|e| is_state(e, PlayerStatus::Idle))
    }

    fn with_repeat(repeat: RepeatMode) -> PlayerOptions {
        PlayerOptions {
            repeat,
            ..PlayerOptions::default()
        }
    }

    fn gap_tracks() -> Vec<Track> {
        vec![
            fixture_track("a", "gap_a.wav"),
            fixture_track("b", "gap_b.wav"),
        ]
    }

    /// No `Loading` after the first track change: every later one was a hand-over.
    fn handed_over(events: &[PlayerEvent]) -> bool {
        let first = events
            .iter()
            .position(|e| matches!(e, PlayerEvent::TrackChanged(_)))
            .unwrap();
        !events[first..]
            .iter()
            .any(|e| is_state(e, PlayerStatus::Loading))
    }

    /// `played` starts with `runs` back to back, then goes on with a prefix of `then`.
    fn assert_continuous(played: &[f32], runs: &[Vec<f32>], then: &[f32]) {
        let expected: Vec<f32> = runs.concat();
        assert!(played.len() >= expected.len(), "too little was played");
        assert!(
            played[..expected.len()] == expected[..],
            "one continuous run across the boundaries"
        );
        let rest = &played[expected.len()..];
        let n = rest.len().min(then.len());
        assert!(rest[..n] == then[..n], "the wrap continues at the start");
    }

    #[test]
    fn repeat_all_wraps_to_the_first_track_gaplessly() {
        let h = harness_options(gap_tracks(), with_repeat(RepeatMode::All));
        h.player.play(ids(&["a", "b"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), |seen| {
            changes(seen).len() == 3
        });
        assert_eq!(changes(&events), ids(&["a", "b", "a"]));
        assert!(handed_over(&events), "the wrap was pre-opened: {events:#?}");
        let played = h.played.lock().unwrap().clone();
        let (a, b) = (decode_all("gap_a.wav"), decode_all("gap_b.wav"));
        assert_continuous(&played, &[a.clone(), b], &a);
        let state = h.player.state();
        assert_eq!(state.repeat, RepeatMode::All);
        assert_ne!(state.status, PlayerStatus::Idle, "the queue never ends");
    }

    #[test]
    fn repeat_all_crossfades_across_the_wrap() {
        let mut a = album_track("a", "x", 1);
        a.duration_ms = 2_000;
        let mut c = album_track("c", "y", 2);
        c.duration_ms = 2_000;
        let tracks = [a, c];
        let resolver: Resolver =
            Arc::new(move |id: &str| tracks.iter().find(|track| track.id == id).cloned());
        let opener = tone_opener(vec![("a", TONE_A, 2_000), ("c", TONE_C, 2_000)]);
        let options = PlayerOptions {
            repeat: RepeatMode::All,
            ..crossfade_always(1_000)
        };
        let h = harness_full(resolver, 5.0, Some(opener), options);
        h.player.play(ids(&["a", "c"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), |seen| {
            changes(seen).len() == 4
        });
        assert_eq!(changes(&events), ids(&["a", "c", "a", "c"]));
        let played = h.played.lock().unwrap().clone();
        // Where c ends and a starts again, the two are mixed as at a -> c.
        let first_c = played
            .iter()
            .position(|s| (*s - TONE_C).abs() < 1e-6)
            .unwrap();
        let after_c = &played[first_c..];
        let second_a = after_c
            .iter()
            .position(|s| (*s - TONE_A).abs() < 1e-6)
            .expect("a again");
        let blended = after_c[..second_a]
            .iter()
            .filter(|s| (**s - TONE_A).abs() > 1e-6 && (**s - TONE_C).abs() > 1e-6)
            .count();
        assert!(blended > 2 * 35_000, "{blended} samples mixed at the wrap");
    }

    #[test]
    fn repeat_one_replays_the_current_track_gaplessly() {
        let h = harness_options(gap_tracks(), with_repeat(RepeatMode::One));
        h.player.play(ids(&["a", "b"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), |seen| {
            changes(seen).len() == 3
        });
        assert_eq!(changes(&events), ids(&["a", "a", "a"]));
        assert!(
            handed_over(&events),
            "the repeat was pre-opened: {events:#?}"
        );
        let played = h.played.lock().unwrap().clone();
        let a = decode_all("gap_a.wav");
        assert_continuous(&played, &[a.clone(), a.clone()], &a);
        assert_eq!(h.player.state().queue_index, 0);
    }

    #[test]
    fn repeat_one_with_gapless_off_replays_after_the_end() {
        let options = PlayerOptions {
            gapless: false,
            ..with_repeat(RepeatMode::One)
        };
        let h = harness_options(gap_tracks(), options);
        h.player.play(ids(&["a", "b"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), |seen| {
            changes(seen).len() == 2
        });
        assert_eq!(changes(&events), ids(&["a", "a"]));
        assert!(!handed_over(&events), "loaded again after the end");
    }

    #[test]
    fn changing_repeat_drops_a_preloaded_next_track() {
        let mut a = album_track("a", "x", 1);
        a.duration_ms = 2_000;
        let mut b = album_track("b", "y", 1);
        b.duration_ms = 2_000;
        let tracks = [a, b];
        let resolver: Resolver =
            Arc::new(move |id: &str| tracks.iter().find(|track| track.id == id).cloned());
        let opener = tone_opener(vec![("a", TONE_A, 2_000), ("b", TONE_C, 2_000)]);
        let h = harness_full(resolver, 5.0, Some(opener), PlayerOptions::default());
        h.player.play(ids(&["a", "b"]), 0).unwrap();
        collect_until(&h.events, Duration::from_secs(10), |seen| {
            changes(seen) == ids(&["a"])
        });
        // "b" is pre-opened at once (2 s is within the 5 s preload window).
        h.player.set_repeat(RepeatMode::One).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), |seen| {
            !changes(seen).is_empty()
        });
        assert_eq!(changes(&events), ids(&["a"]), "a again, not b");
        h.player.set_repeat(RepeatMode::Off).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), ends_idle);
        assert_eq!(changes(&events), ids(&["b"]));
        assert_eq!(
            played_of(&h.played, TONE_A),
            2 * 2 * 88_200,
            "a twice, whole"
        );
        assert_eq!(played_of(&h.played, TONE_C), 2 * 88_200, "b once, whole");
    }

    #[test]
    fn manual_next_follows_the_repeat_mode() {
        // One: next goes to the next track.
        let h = harness_full(
            {
                let tracks = long_tracks(&["a", "b"]);
                Arc::new(move |id: &str| tracks.iter().find(|t| t.id == id).cloned())
            },
            1.0,
            Some(silence_opener()),
            with_repeat(RepeatMode::One),
        );
        h.player.play(ids(&["a", "b"]), 0).unwrap();
        wait_state(&h.player, |s| playing_track(s, "a"));
        h.player.next().unwrap();
        wait_state(&h.player, |s| playing_track(s, "b"));
        // All: next at the end wraps to the first track.
        h.player.set_repeat(RepeatMode::All).unwrap();
        h.player.next().unwrap();
        let state = wait_state(&h.player, |s| playing_track(s, "a"));
        assert_eq!(state.queue_index, 0);
        // All: prev at the start (within 3 s) wraps to the last track.
        h.player.prev().unwrap();
        let state = wait_state(&h.player, |s| playing_track(s, "b"));
        assert_eq!(state.queue_index, 1);
        // Off: next at the end stops.
        h.player.set_repeat(RepeatMode::Off).unwrap();
        h.player.next().unwrap();
        wait_state(&h.player, |s| s.status == PlayerStatus::Idle);
    }

    #[test]
    fn repeat_all_on_a_dead_queue_still_gives_up() {
        let h = harness_options(Vec::new(), with_repeat(RepeatMode::All));
        h.player.play(ids(&["gone1", "gone2", "gone3"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), |seen| {
            errors(seen).len() == MAX_FAILURES_IN_A_ROW as usize
                && seen
                    .iter()
                    .rev()
                    .take_while(|e| !matches!(e, PlayerEvent::Error(_)))
                    .any(|e| is_state(e, PlayerStatus::Idle))
        });
        assert_eq!(errors(&events).len(), 10);
        std::thread::sleep(Duration::from_millis(300));
        assert!(errors(&h.events.try_iter().collect::<Vec<_>>()).is_empty());
        assert_eq!(h.player.state().status, PlayerStatus::Idle);
    }

    #[test]
    fn repeat_one_skips_a_track_that_fails() {
        let h = harness_options(
            vec![
                fixture_track("bad", "corrupt.flac"),
                fixture_track("a", "gap_a.wav"),
            ],
            with_repeat(RepeatMode::One),
        );
        h.player.play(ids(&["bad", "a"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), |seen| {
            changes(seen).len() == 2
        });
        assert_eq!(errors(&events), ids(&["bad"]), "reported once, not retried");
        assert_eq!(changes(&events), ids(&["a", "a"]));
    }

    #[test]
    fn repeat_and_shuffle_are_ignored_in_flow() {
        let options = PlayerOptions {
            shuffle: true,
            ..with_repeat(RepeatMode::All)
        };
        let h = harness_options(gap_tracks(), options);
        let flow = Some(FlowVariant::Library);
        h.player
            .handle()
            .play_with(ids(&["a", "b"]), 0, flow)
            .unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), back_to_idle);
        assert_eq!(changes(&events), ids(&["a", "b"]), "in order, once");
        let state = h.player.state();
        assert_eq!(state.repeat, RepeatMode::All, "the mode is kept for later");
        assert!(state.shuffle);

        let ten: Vec<String> = (0..10).map(|n| format!("t{n}")).collect();
        h.player
            .handle()
            .play_with(ten.clone(), 0, Some(FlowVariant::Library))
            .unwrap();
        wait_state(&h.player, |s| s.queue_length == 10);
        h.player.set_shuffle(false).unwrap();
        h.player.set_shuffle(true).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(h.player.queue_ids(), ten, "a Flow is never reshuffled");
    }

    fn ten_long() -> Harness {
        let names: Vec<String> = (0..10).map(|n| format!("t{n}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        harness(long_tracks(&refs), 1.0, Some(silence_opener()))
    }

    fn ten_ids() -> Vec<String> {
        (0..10).map(|n| format!("t{n}")).collect()
    }

    #[test]
    fn shuffle_on_and_off_while_playing() {
        let h = ten_long();
        h.player.play(ten_ids(), 2).unwrap();
        let before = wait_state(&h.player, |s| playing_track(s, "t2"));
        assert!(!before.shuffle);
        h.player.set_shuffle(true).unwrap();
        let on = wait_state(&h.player, |s| s.shuffle);
        assert!(on.queue_version > before.queue_version);
        assert!(playing_track(&on, "t2"), "keeps playing: {on:#?}");
        assert_eq!(on.queue_index, 2);
        let shuffled = h.player.queue_ids();
        assert_eq!(&shuffled[..3], &ten_ids()[..3]);
        assert_ne!(shuffled, ten_ids());

        // Play on into the shuffled part, then back to the original order.
        h.player.next().unwrap();
        let moved_on = wait_state(&h.player, |s| {
            s.queue_index == 3 && s.status == PlayerStatus::Playing
        });
        let now = moved_on.track.unwrap().id;
        assert_eq!(now, shuffled[3]);
        h.player.set_shuffle(false).unwrap();
        let off = wait_state(&h.player, |s| !s.shuffle);
        assert_eq!(h.player.queue_ids(), ten_ids());
        assert!(playing_track(&off, &now));
        assert_eq!(
            off.queue_index as usize,
            ten_ids().iter().position(|id| *id == now).unwrap()
        );
    }

    #[test]
    fn play_while_shuffled_starts_with_the_chosen_track() {
        let names = ten_ids();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let options = PlayerOptions {
            shuffle: true,
            ..PlayerOptions::default()
        };
        let resolver: Resolver = {
            let tracks = long_tracks(&refs);
            Arc::new(move |id: &str| tracks.iter().find(|t| t.id == id).cloned())
        };
        let h = harness_full(resolver, 1.0, Some(silence_opener()), options);
        h.player.play(ten_ids(), 4).unwrap();
        let state = wait_state(&h.player, |s| playing_track(s, "t4"));
        assert_eq!(state.queue_index, 0);
        let mut all = h.player.queue_ids();
        assert_eq!(all[0], "t4");
        all.sort();
        assert_eq!(all, ten_ids());

        // Appends land in the upcoming part.
        h.player.append(ids(&["x", "y"])).unwrap();
        wait_state(&h.player, |s| s.queue_length == 12);
        assert_eq!(h.player.queue_ids()[0], "t4");
        assert!(playing_track(&h.player.state(), "t4"));
    }

    #[test]
    fn move_keeps_the_playing_track_and_bumps_the_version() {
        let h = harness(
            long_tracks(&["a", "b", "c", "d"]),
            1.0,
            Some(silence_opener()),
        );
        h.player.play(ids(&["a", "b", "c", "d"]), 1).unwrap();
        let before = wait_state(&h.player, |s| playing_track(s, "b"));
        h.player.move_entry(3, 0).unwrap();
        let after = wait_state(&h.player, |s| s.queue_version > before.queue_version);
        assert_eq!(h.player.queue_ids(), ids(&["d", "a", "b", "c"]));
        assert_eq!(after.queue_index, 2);
        assert!(playing_track(&after, "b"), "not restarted: {after:#?}");
        // Out of range: ignored.
        h.player.move_entry(9, 0).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(h.player.state().queue_version, after.queue_version);
    }

    #[test]
    fn remove_keeps_the_playing_track_and_skips_a_removed_current_one() {
        let h = harness(long_tracks(&["a", "b", "c"]), 1.0, Some(silence_opener()));
        h.player.play(ids(&["a", "b", "c"]), 1).unwrap();
        let before = wait_state(&h.player, |s| playing_track(s, "b"));
        h.player.remove(0).unwrap();
        let after = wait_state(&h.player, |s| s.queue_version > before.queue_version);
        assert_eq!(h.player.queue_ids(), ids(&["b", "c"]));
        assert_eq!(after.queue_index, 0);
        assert!(playing_track(&after, "b"));
        // The playing one: the next one plays.
        h.player.remove(0).unwrap();
        let state = wait_state(&h.player, |s| playing_track(s, "c"));
        assert_eq!(state.queue_index, 0);
        assert_eq!(state.queue_length, 1);
        // The last one: idle with an empty queue.
        h.player.remove(0).unwrap();
        let state = wait_state(&h.player, |s| s.queue_length == 0);
        assert_eq!(state.status, PlayerStatus::Idle);
    }

    #[test]
    fn remove_the_playing_track_while_paused_stays_paused_on_the_next() {
        let h = harness(long_tracks(&["a", "b", "c"]), 1.0, Some(silence_opener()));
        h.player.play(ids(&["a", "b", "c"]), 0).unwrap();
        wait_state(&h.player, |s| playing_track(s, "a"));
        h.player.toggle().unwrap();
        wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
        h.player.remove(0).unwrap();
        let state = wait_state(&h.player, |s| {
            s.status == PlayerStatus::Paused && s.track.as_ref().is_some_and(|t| t.id == "b")
        });
        assert_eq!(state.queue_index, 0);
        std::thread::sleep(Duration::from_millis(200));
        let state = h.player.state();
        assert_eq!(state.status, PlayerStatus::Paused, "{state:#?}");
        assert_eq!(state.track.unwrap().id, "b");
    }

    #[test]
    fn remove_the_next_track_during_a_hand_over_drops_it_and_keeps_the_tail() {
        const TONE_B: f32 = 0.125;
        let tracks: Vec<Track> = ["a", "b", "c"]
            .iter()
            .map(|id| {
                let mut track = fixture_track(id, "tagged.flac");
                track.duration_ms = TONE_A_MS;
                track
            })
            .collect();
        let resolver: Resolver =
            Arc::new(move |id: &str| tracks.iter().find(|track| track.id == id).cloned());
        let opener = tone_opener(vec![
            ("a", TONE_A, TONE_A_MS),
            ("b", TONE_B, TONE_A_MS),
            ("c", TONE_C, TONE_A_MS),
        ]);
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
        player.play(ids(&["a", "b", "c"]), 0).unwrap();
        // Once the device has played more than "a" minus a full ring, "a" is decoded and
        // "b" has taken over; "a"'s tail (about 1.9 s) is still to be heard.
        let decoded = TONE_A_SAMPLES - (BUFFERED_RING_MS * 44_100 / 1000) as usize * 2;
        let deadline = Instant::now() + Duration::from_secs(20);
        while played_of(&played, TONE_A) <= decoded + 4_410 {
            assert!(Instant::now() < deadline, "a never played");
            std::thread::sleep(Duration::from_millis(5));
        }
        let state = player.state();
        assert_eq!(
            state.track.unwrap().id,
            "a",
            "the hand-over is still pending"
        );
        assert_eq!(state.queue_index, 0);
        // The UI's next row.
        player.remove(1).unwrap();
        let events = collect_until(&events, Duration::from_secs(20), back_to_idle);
        assert_eq!(
            changes(&events),
            ids(&["a", "c"]),
            "b was never announced: {events:#?}"
        );
        assert_eq!(player.queue_ids(), ids(&["a", "c"]));
        assert_eq!(
            played_of(&played, TONE_A),
            TONE_A_SAMPLES,
            "a's tail was not cut"
        );
        assert!(played_of(&played, TONE_B) < TONE_A_SAMPLES / 2);
        assert_eq!(played_of(&played, TONE_C), TONE_A_SAMPLES);
    }

    #[test]
    fn remove_before_a_loading_track_does_not_lose_it() {
        let tracks = long_tracks(&["a", "b", "c"]);
        let (opening_tx, opening) = crossbeam_channel::unbounded();
        let (go_tx, go) = crossbeam_channel::unbounded::<()>();
        let opener: Opener = Arc::new(move |track: &Track, _spec: StreamSpec| {
            if track.id == "c" && std::thread::current().name() == Some(OPENER_CURRENT_THREAD) {
                let _ = opening_tx.send(());
                let _ = go.recv_timeout(Duration::from_secs(5));
            }
            Ok(Box::new(Silence {
                frames_left: SILENCE_MS * 44_100 / 1000,
            }) as Box<dyn AudioSource>)
        });
        let h = harness(tracks, 1.0, Some(opener));
        h.player.play(ids(&["a", "b", "c"]), 2).unwrap();
        opening.recv_timeout(Duration::from_secs(5)).unwrap();
        h.player.remove(0).unwrap();
        wait_state(&h.player, |s| s.queue_length == 2);
        go_tx.send(()).unwrap();
        let state = wait_state(&h.player, |s| playing_track(s, "c"));
        assert_eq!(state.queue_index, 1);
    }

    #[test]
    fn a_move_changes_the_preloaded_next_track() {
        let mut a = album_track("a", "x", 1);
        a.duration_ms = 2_000;
        let mut b = album_track("b", "y", 1);
        b.duration_ms = 1_000;
        let mut c = album_track("c", "z", 1);
        c.duration_ms = 1_000;
        let tracks = [a, b, c];
        let resolver: Resolver =
            Arc::new(move |id: &str| tracks.iter().find(|track| track.id == id).cloned());
        const TONE_B: f32 = 0.125;
        let opener = tone_opener(vec![
            ("a", TONE_A, 2_000),
            ("b", TONE_B, 1_000),
            ("c", TONE_C, 1_000),
        ]);
        let h = harness_full(resolver, 4.0, Some(opener), PlayerOptions::default());
        h.player.play(ids(&["a", "b", "c"]), 0).unwrap();
        collect_until(&h.events, Duration::from_secs(10), |seen| {
            changes(seen) == ids(&["a"])
        });
        h.player.move_entry(2, 1).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), ends_idle);
        assert_eq!(changes(&events), ids(&["c", "b"]));
        assert!(
            !events.iter().any(|e| is_state(e, PlayerStatus::Loading)),
            "c was pre-opened after the move: {events:#?}"
        );
        assert_eq!(played_of(&h.played, TONE_A), 2 * 88_200);
        assert_eq!(played_of(&h.played, TONE_B), 88_200);
        assert_eq!(played_of(&h.played, TONE_C), 88_200);
    }

    fn levels(events: &[PlayerEvent]) -> Vec<f32> {
        events
            .iter()
            .filter_map(|event| match event {
                PlayerEvent::Level(level) => Some(*level),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn level_is_sent_every_50_ms_while_playing_without_extra_states() {
        let opener: Opener = Arc::new(|_track: &Track, _spec: StreamSpec| {
            Ok(Box::new(Tone::new(0.5, SILENCE_MS)) as Box<dyn AudioSource>)
        });
        let h = harness(long_tracks(&["a"]), 1.0, Some(opener));
        h.player.play(ids(&["a"]), 0).unwrap();
        wait_state(&h.player, |s| s.status == PlayerStatus::Playing);
        std::thread::sleep(Duration::from_millis(200));
        h.events.try_iter().for_each(drop);

        std::thread::sleep(Duration::from_millis(1_000));
        let second: Vec<PlayerEvent> = h.events.try_iter().collect();
        let sent = levels(&second);
        assert!(
            (14..=22).contains(&sent.len()),
            "{} levels in 1 s",
            sent.len()
        );
        // A constant 0.5 (-6 dBFS) reads as 0.88.
        let expected = (20.0 * 0.5_f32.log10() + 50.0) / 50.0;
        assert!(
            sent.iter().all(|level| (level - expected).abs() <= 0.02),
            "{sent:?}"
        );
        let states = states(&second);
        assert!(
            (3..=5).contains(&states.len()),
            "only the periodic states: {states:#?}"
        );
        assert_eq!(h.player.state().status, PlayerStatus::Playing);

        // Paused: no more levels.
        h.player.toggle().unwrap();
        wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
        std::thread::sleep(Duration::from_millis(60));
        h.events.try_iter().for_each(drop);
        std::thread::sleep(Duration::from_millis(300));
        let paused: Vec<PlayerEvent> = h.events.try_iter().collect();
        assert!(levels(&paused).is_empty(), "{paused:#?}");
    }

    /// What the fake Navidrome of [`NetServer`] does right now.
    #[derive(Clone, Default)]
    struct NetConfig {
        /// Every open fails with `Offline` (the server is down).
        down: bool,
        /// Opens of these ids fail with `Offline`.
        down_ids: Vec<String>,
        /// Opens of these ids fail with a plain error (not audio).
        fail_ids: Vec<String>,
        /// The stream of an id drops (`Offline`) once it reaches this position (ms).
        cut_at: Vec<(String, u64)>,
        /// The stream of an id fails with a plain decode error at this position (ms).
        broken_at: Vec<(String, u64)>,
        /// The stream of an id ends now, without an error (a station that closed it).
        ended: Vec<String>,
        /// Every open takes this long, and opens on the next-track worker this much more.
        open_delay: Duration,
        next_open_delay: Duration,
        /// Every read takes this long (and returns up to [`NET_READ_FRAMES`]).
        read_delay: Duration,
    }

    /// Frames one [`NetSource`] read returns at most (about 93 ms).
    const NET_READ_FRAMES: u64 = 4_096;

    /// A fake Navidrome for the engine: its tracks are constant tones, one value per id.
    #[derive(Default)]
    struct NetServer {
        config: Mutex<NetConfig>,
        /// (id, ms) of every seek, including the one an open at a position makes.
        seeks: Mutex<Vec<(String, u64)>>,
        /// The ids opened, in order.
        opens: Mutex<Vec<String>>,
        /// (id, opener thread) of every open.
        open_threads: Mutex<Vec<(String, String)>>,
        /// The ids whose source was dropped (its connection closed), in order.
        drops: Mutex<Vec<String>>,
    }

    impl NetServer {
        fn new() -> Arc<NetServer> {
            Arc::new(NetServer::default())
        }

        fn set(&self, change: impl FnOnce(&mut NetConfig)) {
            change(&mut self.config.lock().unwrap());
        }

        fn config(&self) -> NetConfig {
            self.config.lock().unwrap().clone()
        }

        fn seeks(&self) -> Vec<(String, u64)> {
            self.seeks.lock().unwrap().clone()
        }

        fn opens(&self) -> Vec<String> {
            self.opens.lock().unwrap().clone()
        }

        fn open_threads(&self) -> Vec<(String, String)> {
            self.open_threads.lock().unwrap().clone()
        }

        fn drops(&self) -> Vec<String> {
            self.drops.lock().unwrap().clone()
        }

        /// Opens `tones` (id, value, ms) as [`NetSource`]s.
        fn opener(self: &Arc<Self>, tones: Vec<(&'static str, f32, u64)>) -> Opener {
            let server = Arc::clone(self);
            Arc::new(move |track: &Track, _spec: StreamSpec| {
                let config = server.config();
                let mut delay = config.open_delay;
                if std::thread::current().name() == Some(OPENER_NEXT_THREAD) {
                    delay += config.next_open_delay;
                }
                std::thread::sleep(delay);
                server.opens.lock().unwrap().push(track.id.clone());
                let thread = std::thread::current().name().unwrap_or("").to_string();
                server
                    .open_threads
                    .lock()
                    .unwrap()
                    .push((track.id.clone(), thread));
                if config.down || config.down_ids.contains(&track.id) {
                    return Err(anyhow::Error::new(Offline("connection refused".into())));
                }
                if config.fail_ids.contains(&track.id) {
                    anyhow::bail!("this station is not sending audio");
                }
                let (_, value, ms) = tones
                    .iter()
                    .find(|(id, _, _)| *id == track.id)
                    .copied()
                    .ok_or_else(|| anyhow::anyhow!("no tone for {}", track.id))?;
                let live = is_live(track);
                Ok(Box::new(NetSource {
                    id: track.id.clone(),
                    server: Arc::clone(&server),
                    value,
                    frames: if live { u64::MAX } else { ms * 44_100 / 1000 },
                    position: 0,
                    live,
                }) as Box<dyn AudioSource>)
            })
        }
    }

    /// A stream of the fake Navidrome: a tone that reads slowly and can drop mid-track. A
    /// station's stream (`live`) is endless and cannot seek.
    struct NetSource {
        id: String,
        server: Arc<NetServer>,
        value: f32,
        frames: u64,
        /// Frames read so far.
        position: u64,
        live: bool,
    }

    impl Drop for NetSource {
        fn drop(&mut self) {
            if let Ok(mut drops) = self.server.drops.lock() {
                drops.push(self.id.clone());
            }
        }
    }

    impl NetSource {
        /// The first frame at or after `ms` of this id in `list`.
        fn frame_of(list: &[(String, u64)], id: &str) -> Option<u64> {
            list.iter()
                .find(|(each, _)| each == id)
                .map(|(_, ms)| ms * 44_100 / 1000)
        }
    }

    impl AudioSource for NetSource {
        fn spec(&self) -> StreamSpec {
            STEREO_44K
        }
        fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize> {
            let config = self.server.config();
            std::thread::sleep(config.read_delay);
            let cut = Self::frame_of(&config.cut_at, &self.id);
            let broken = Self::frame_of(&config.broken_at, &self.id);
            if cut.is_some_and(|at| self.position >= at) {
                return Err(anyhow::Error::new(Offline("the connection dropped".into())));
            }
            if broken.is_some_and(|at| self.position >= at) {
                anyhow::bail!("a broken frame");
            }
            if config.ended.contains(&self.id) {
                return Ok(0);
            }
            let stop = [Some(self.frames), cut, broken]
                .into_iter()
                .flatten()
                .min()
                .unwrap_or(self.frames);
            let frames = stop.saturating_sub(self.position).min(NET_READ_FRAMES);
            out.extend(std::iter::repeat_n(self.value, frames as usize * 2));
            self.position += frames;
            Ok(frames as usize)
        }
        fn seek(&mut self, ms: u64) -> anyhow::Result<()> {
            self.server
                .seeks
                .lock()
                .unwrap()
                .push((self.id.clone(), ms));
            if self.live {
                anyhow::bail!("a live station cannot seek");
            }
            self.position = (ms * 44_100 / 1000).min(self.frames);
            Ok(())
        }
        fn duration_ms(&self) -> Option<u64> {
            (!self.live).then(|| self.frames * 1000 / 44_100)
        }
    }

    /// Navidrome tracks (no file path) of the given lengths; a `radio:` id is a station.
    fn net_tracks(tracks: &[(&str, u64)]) -> Vec<Track> {
        tracks
            .iter()
            .map(|(id, ms)| {
                if id.starts_with(RADIO_PREFIX) {
                    let mut track = test_station(1).as_track();
                    track.id = id.to_string();
                    return track;
                }
                let mut track = fixture_track(id, "tagged.flac");
                track.source = Source::Navidrome;
                track.path = None;
                track.duration_ms = *ms;
                track
            })
            .collect()
    }

    fn net_harness(
        tracks: &[(&'static str, f32, u64)],
        server: &Arc<NetServer>,
        speed: f64,
        options: PlayerOptions,
    ) -> Harness {
        net_harness_with(tracks, server, speed, options, None)
    }

    /// Like [`net_harness`]; every event also goes to `extra` (the history recorder).
    fn net_harness_with(
        tracks: &[(&'static str, f32, u64)],
        server: &Arc<NetServer>,
        speed: f64,
        options: PlayerOptions,
        extra: Option<EventSink>,
    ) -> Harness {
        let lengths: Vec<(&str, u64)> = tracks.iter().map(|(id, _, ms)| (*id, *ms)).collect();
        let known = net_tracks(&lengths);
        let resolver: Resolver =
            Arc::new(move |id: &str| known.iter().find(|track| track.id == id).cloned());
        // An output that holds a ring like the real one, so a hand-over is heard later than
        // it is decoded.
        let played = Arc::new(Mutex::new(Vec::new()));
        let factory = Arc::new(BufferedFactory {
            speed,
            ring_ms: u64::from(RING_MS),
            played: Arc::clone(&played),
        });
        let (tx, events) = crossbeam_channel::unbounded();
        let mut sink: EventSink = Arc::new(move |event| {
            let _ = tx.send(event);
        });
        if let Some(extra) = extra {
            sink = fan_out(vec![sink, extra]);
        }
        let opener = server.opener(tracks.to_vec());
        let player = Player::spawn_with_options(factory, resolver, sink, opener, options);
        Harness {
            player,
            events,
            played,
        }
    }

    /// (track id, kind) of every error.
    fn error_kinds(events: &[PlayerEvent]) -> Vec<(String, ErrorKind)> {
        events
            .iter()
            .filter_map(|event| match event {
                PlayerEvent::Error(error) => Some((error.track_id.clone(), error.kind)),
                _ => None,
            })
            .collect()
    }

    fn offline(id: &str) -> (String, ErrorKind) {
        (id.to_string(), ErrorKind::Offline)
    }

    /// Sets a [`NetConfig`] up for a test case.
    type NetSetup = fn(&mut NetConfig);

    const NET_A: f32 = 0.25;
    const NET_B: f32 = 0.5;
    const NET_C: f32 = 0.75;

    /// Samples (stereo) of `ms` at 44.1 kHz.
    fn samples_of(ms: u64) -> usize {
        (ms * 44_100 / 1000) as usize * 2
    }

    #[test]
    fn offline_open_pauses_on_that_track() {
        for repeat in [RepeatMode::Off, RepeatMode::All, RepeatMode::One] {
            let server = NetServer::new();
            server.set(|c| c.down = true);
            let tones = [("n1", NET_A, 2_000), ("n2", NET_B, 2_000)];
            let h = net_harness(&tones, &server, FAST, with_repeat(repeat));
            h.player.play(ids(&["n1", "n2"]), 0).unwrap();
            let events = collect_until(&h.events, Duration::from_secs(5), |seen| {
                !errors(seen).is_empty()
            });
            let state = wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
            assert_eq!(error_kinds(&events), vec![offline("n1")], "{repeat:?}");
            let message = events.iter().find_map(|e| match e {
                PlayerEvent::Error(error) => Some(error.message.clone()),
                _ => None,
            });
            assert_eq!(message.as_deref(), Some(OFFLINE_MESSAGE));
            assert_eq!(state.track.as_ref().unwrap().id, "n1", "{repeat:?}");
            assert_eq!(state.queue_index, 0, "{repeat:?}");
            assert_eq!(state.position_ms, 0);
            assert!(changes(&events).is_empty(), "nothing was heard");
            // It stays there: no retries, no skipping.
            std::thread::sleep(Duration::from_millis(300));
            let later: Vec<PlayerEvent> = h.events.try_iter().collect();
            assert!(errors(&later).is_empty(), "{repeat:?}: {later:#?}");
            assert_eq!(server.opens(), ids(&["n1"]), "{repeat:?}");
            let state = h.player.state();
            assert_eq!(state.status, PlayerStatus::Paused);
            assert_eq!(state.queue_index, 0);
        }
    }

    /// Plays n1 (10 s) of n1, n2, n3 until its stream drops at 3 s; the paused state.
    fn play_until_cut(server: &Arc<NetServer>) -> (Harness, Vec<PlayerEvent>, PlayerState) {
        play_until_cut_with(server, None)
    }

    /// [`play_until_cut`], with every event also going to `extra`.
    fn play_until_cut_with(
        server: &Arc<NetServer>,
        extra: Option<EventSink>,
    ) -> (Harness, Vec<PlayerEvent>, PlayerState) {
        server.set(|c| c.cut_at = vec![("n1".into(), 3_000)]);
        let tones = [
            ("n1", NET_A, SILENCE_MS),
            ("n2", NET_B, 1_000),
            ("n3", NET_C, 1_000),
        ];
        let h = net_harness_with(&tones, server, FAST, PlayerOptions::default(), extra);
        h.player.play(ids(&["n1", "n2", "n3"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), |seen| {
            !errors(seen).is_empty()
        });
        let state = wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
        (h, events, state)
    }

    #[test]
    fn offline_mid_track_pauses_at_position() {
        let server = NetServer::new();
        let (h, events, state) = play_until_cut(&server);
        assert_eq!(error_kinds(&events), vec![offline("n1")]);
        assert_eq!(changes(&events), ids(&["n1"]));
        assert_eq!(state.track.as_ref().unwrap().id, "n1");
        assert_eq!(state.queue_index, 0);
        assert!(
            state.position_ms.abs_diff(3_000) <= 300,
            "paused at {} ms",
            state.position_ms
        );
        let heard = played_of(&h.played, NET_A);
        assert!(
            heard.abs_diff(samples_of(3_000)) <= samples_of(300),
            "{heard} samples of n1 heard"
        );
        std::thread::sleep(Duration::from_millis(300));
        let later: Vec<PlayerEvent> = h.events.try_iter().collect();
        assert!(errors(&later).is_empty(), "one notice: {later:#?}");
        assert!(
            changes(&later).is_empty(),
            "the queue did not move: {later:#?}"
        );
        // Nothing is pre-opened behind a stream that failed.
        assert_eq!(server.opens(), ids(&["n1"]), "n2 was never opened");
        assert_eq!(played_of(&h.played, NET_B), 0);
        assert_eq!(h.player.state().status, PlayerStatus::Paused);
    }

    #[test]
    fn resume_after_offline_reopens_at_position() {
        let server = NetServer::new();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (h, before, paused) =
            play_until_cut_with(&server, Some(spawn_recorder(Arc::clone(&db))));
        let held = paused.position_ms;
        server.set(|c| c.cut_at.clear());
        h.player.toggle().unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), |seen| {
            changes(seen).contains(&"n2".to_string())
        });
        assert!(errors(&events).is_empty(), "{events:#?}");
        assert_eq!(
            server.seeks(),
            vec![("n1".to_string(), held)],
            "reopened at the held position"
        );
        let opens = server.opens();
        assert_eq!(
            opens.iter().filter(|id| *id == "n1").count(),
            2,
            "{opens:?}"
        );
        // n1 resumed where it stopped: about 3 s before the cut, 7 s after it.
        let heard = played_of(&h.played, NET_A);
        assert!(
            heard.abs_diff(samples_of(SILENCE_MS)) <= samples_of(300),
            "{heard} samples of n1 heard"
        );
        let resumed = states(&events)
            .into_iter()
            .find(|s| playing_track(s, "n1"))
            .expect("n1 played again");
        assert!(resumed.position_ms >= held, "{resumed:#?}");
        // The resume is the same play: n1 was announced once, and the history has one play.
        let mut all = before;
        all.extend(events);
        assert_eq!(changes(&all), ids(&["n1", "n2"]), "{all:#?}");
        let plays = |id: &str| -> i64 {
            db.0.lock()
                .unwrap()
                .query_row(
                    "SELECT COALESCE(SUM(play_count), 0) FROM track_stats WHERE track_id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .unwrap()
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while plays("n2") < 1 {
            assert!(Instant::now() < deadline, "the history never saw n2");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            plays("n1"),
            1,
            "one play of n1 across the cut and the resume"
        );
    }

    #[test]
    fn offline_preopen_tried_again_when_the_server_is_back() {
        let server = NetServer::new();
        server.set(|c| c.down_ids = vec!["n2".into()]);
        let tones = [("n1", NET_A, 1_500), ("n2", NET_B, 1_000)];
        let h = net_harness(&tones, &server, 1.0, PlayerOptions::default());
        h.player.play(ids(&["n1", "n2"]), 0).unwrap();
        // n1 is shorter than the pre-open lead: n2 is pre-opened at once, and fails.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !server.opens().contains(&"n2".to_string()) {
            assert!(Instant::now() < deadline, "n2 was never pre-opened");
            std::thread::sleep(Duration::from_millis(5));
        }
        server.set(|c| c.down_ids.clear());
        let events = collect_until(&h.events, Duration::from_secs(10), back_to_idle);
        assert!(errors(&events).is_empty(), "no notice: {events:#?}");
        assert_eq!(changes(&events), ids(&["n1", "n2"]));
        assert_eq!(played_of(&h.played, NET_B), samples_of(1_000));
        assert_eq!(server.opens(), ids(&["n1", "n2", "n2"]));
    }

    #[test]
    fn a_track_held_at_open_is_announced_once_when_it_plays() {
        let server = NetServer::new();
        server.set(|c| c.down = true);
        let tones = [("n1", NET_A, 1_000)];
        let h = net_harness(&tones, &server, FAST, PlayerOptions::default());
        h.player.play(ids(&["n1"]), 0).unwrap();
        let mut events = collect_until(&h.events, Duration::from_secs(5), |seen| {
            !errors(seen).is_empty()
        });
        wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
        assert!(
            changes(&events).is_empty(),
            "never audible, never announced"
        );
        server.set(|c| c.down = false);
        h.player.toggle().unwrap();
        events.extend(collect_until(
            &h.events,
            Duration::from_secs(5),
            back_to_idle,
        ));
        assert_eq!(changes(&events), ids(&["n1"]), "one play: {events:#?}");
    }

    #[test]
    fn a_device_change_while_held_stays_held() {
        let server = NetServer::new();
        let (h, _, paused) = play_until_cut(&server);
        h.player.set_device(Some("another output".into())).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let later: Vec<PlayerEvent> = h.events.try_iter().collect();
        assert!(errors(&later).is_empty(), "{later:#?}");
        let state = h.player.state();
        assert_eq!(state.status, PlayerStatus::Paused);
        assert_eq!(state.position_ms, paused.position_ms);
        assert_eq!(state.queue_index, 0);
        assert_eq!(server.opens(), ids(&["n1"]));
    }

    #[test]
    fn offline_errors_do_not_count_toward_the_failure_cap() {
        let server = NetServer::new();
        server.set(|c| c.down = true);
        let tones = [("n1", NET_A, 1_000), ("n2", NET_B, 1_000)];
        let h = net_harness(&tones, &server, FAST, PlayerOptions::default());
        h.player.play(ids(&["n1", "n2"]), 0).unwrap();
        let mut events = collect_until(&h.events, Duration::from_secs(5), |seen| {
            errors(seen).len() == 1
        });
        for round in 2..=12 {
            wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
            h.player.toggle().unwrap();
            events.extend(collect_until(&h.events, Duration::from_secs(5), |seen| {
                !errors(seen).is_empty()
            }));
            assert_eq!(errors(&events).len(), round);
        }
        wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
        assert!(
            error_kinds(&events).iter().all(|e| *e == offline("n1")),
            "{events:#?}"
        );
        server.set(|c| c.down = false);
        h.player.toggle().unwrap();
        let later = collect_until(&h.events, Duration::from_secs(5), |seen| {
            changes(seen) == ids(&["n1"])
        });
        assert!(errors(&later).is_empty(), "{later:#?}");
        wait_state(&h.player, |s| {
            playing_track(s, "n1") || playing_track(s, "n2")
        });
    }

    #[test]
    fn offline_next_track_lets_the_current_one_finish() {
        // n2 cannot even be opened, or it opens and drops before its first frame.
        let cases: [(&str, NetSetup); 2] = [
            ("open", |c| c.down_ids = vec!["n2".into()]),
            ("first read", |c| c.cut_at = vec![("n2".into(), 0)]),
        ];
        for (case, setup) in cases {
            let options = [
                PlayerOptions::default(),
                PlayerOptions {
                    gapless: false,
                    ..PlayerOptions::default()
                },
                crossfade_always(500),
            ];
            for (n, options) in options.into_iter().enumerate() {
                // A fade into a stream that drops at once is cut (as for any failing track).
                if case == "first read" && options.crossfade != Crossfade::Off {
                    continue;
                }
                let gapless = options.gapless;
                let server = NetServer::new();
                server.set(setup);
                let tones = [
                    ("n1", NET_A, 2_000),
                    ("n2", NET_B, 2_000),
                    ("n3", NET_C, 2_000),
                ];
                let h = net_harness(&tones, &server, FAST, options);
                h.player.play(ids(&["n1", "n2", "n3"]), 0).unwrap();
                let events = collect_until(&h.events, Duration::from_secs(10), |seen| {
                    !errors(seen).is_empty()
                });
                let state = wait_state(&h.player, |s| {
                    s.status == PlayerStatus::Paused && s.queue_index == 1
                });
                std::thread::sleep(Duration::from_millis(200));
                let mut events = events;
                events.extend(h.events.try_iter());
                assert_eq!(error_kinds(&events), vec![offline("n2")], "{case} {n}");
                // Without gapless, n2 is loaded after n1's end and shown once it starts (as
                // in M2); its first read then drops. Otherwise it is never shown as heard.
                let shown = if case == "first read" && !gapless {
                    ids(&["n1", "n2"])
                } else {
                    ids(&["n1"])
                };
                assert_eq!(changes(&events), shown, "{case} {n}");
                assert_eq!(state.track.as_ref().unwrap().id, "n2", "{case} {n}");
                assert_eq!(state.position_ms, 0, "{case} {n}");
                assert_eq!(
                    played_of(&h.played, NET_A),
                    samples_of(2_000),
                    "{case} {n}: all of n1 was heard"
                );
                assert_eq!(played_of(&h.played, NET_B), 0, "{case} {n}");
                assert!(
                    !server.opens().contains(&"n3".to_string()),
                    "{case} {n}: n3 was never opened"
                );
                assert_eq!(h.player.state().status, PlayerStatus::Paused);
            }
        }
    }

    #[test]
    fn network_tracks_are_gapless() {
        let server = NetServer::new();
        server.set(|c| c.read_delay = Duration::from_millis(30));
        let tones = [("n1", NET_A, 1_500), ("n2", NET_B, 1_500)];
        let h = net_harness(&tones, &server, 1.0, PlayerOptions::default());
        h.player.play(ids(&["n1", "n2"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), back_to_idle);
        assert!(errors(&events).is_empty(), "{events:#?}");
        assert_eq!(changes(&events), ids(&["n1", "n2"]));
        assert!(handed_over(&events), "n2 was pre-opened: {events:#?}");
        let played = h.played.lock().unwrap().clone();
        let mut expected = vec![NET_A; samples_of(1_500)];
        expected.extend(vec![NET_B; samples_of(1_500)]);
        assert!(played == expected, "one continuous run of n1 then n2");
    }

    #[test]
    fn slow_network_open_of_next_does_not_gap() {
        let server = NetServer::new();
        server.set(|c| c.next_open_delay = Duration::from_secs(1));
        let tones = [("n1", NET_A, 3_000), ("n2", NET_B, 1_000)];
        let h = net_harness(&tones, &server, 1.0, PlayerOptions::default());
        h.player.play(ids(&["n1", "n2"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), back_to_idle);
        assert!(errors(&events).is_empty(), "{events:#?}");
        assert_eq!(changes(&events), ids(&["n1", "n2"]));
        assert!(handed_over(&events), "n2 opened in time: {events:#?}");
        assert_eq!(played_of(&h.played, NET_A), samples_of(3_000));
        assert_eq!(played_of(&h.played, NET_B), samples_of(1_000));
    }

    #[test]
    fn ordinary_errors_still_skip() {
        let server = NetServer::new();
        server.set(|c| c.broken_at = vec![("n1".into(), 500)]);
        let tones = [("n1", NET_A, 2_000), ("n2", NET_B, 1_000)];
        let h = net_harness(&tones, &server, FAST, PlayerOptions::default());
        h.player.play(ids(&["n1", "n2"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), back_to_idle);
        assert_eq!(
            error_kinds(&events),
            vec![("n1".to_string(), ErrorKind::Track)]
        );
        assert_eq!(changes(&events), ids(&["n1", "n2"]));
        assert_eq!(played_of(&h.played, NET_B), samples_of(1_000));
    }

    #[test]
    fn seek_past_the_end_is_clamped_to_the_duration() {
        let server = NetServer::new();
        let tones = [("n1", NET_A, SILENCE_MS), ("n2", NET_B, 1_000)];
        let h = net_harness(&tones, &server, 1.0, PlayerOptions::default());
        h.player.play(ids(&["n1", "n2"]), 0).unwrap();
        wait_state(&h.player, |s| playing_track(s, "n1"));
        h.player.seek(60_000).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), back_to_idle);
        assert!(errors(&events).is_empty(), "{events:#?}");
        assert_eq!(changes(&events), ids(&["n1", "n2"]));
        assert_eq!(server.seeks(), vec![("n1".to_string(), SILENCE_MS)]);
    }

    #[test]
    fn seek_and_prev_while_held_move_the_resume_position() {
        let server = NetServer::new();
        let (h, _, _) = play_until_cut(&server);
        h.player.seek(5_000).unwrap();
        let state = wait_state(&h.player, |s| s.position_ms == 5_000);
        assert_eq!(state.status, PlayerStatus::Paused);
        h.player.prev().unwrap();
        let state = wait_state(&h.player, |s| s.position_ms == 0);
        assert_eq!(
            state.status,
            PlayerStatus::Paused,
            "prev restarts the held track"
        );
        assert_eq!(state.queue_index, 0);
        h.player.seek(60_000).unwrap();
        wait_state(&h.player, |s| s.position_ms == SILENCE_MS);
        h.player.seek(2_000).unwrap();
        wait_state(&h.player, |s| s.position_ms == 2_000);
        server.set(|c| c.cut_at.clear());
        h.player.toggle().unwrap();
        wait_state(&h.player, |s| playing_track(s, "n1"));
        assert_eq!(server.seeks(), vec![("n1".to_string(), 2_000)]);
        let later: Vec<PlayerEvent> = h.events.try_iter().collect();
        assert!(errors(&later).is_empty(), "{later:#?}");
    }

    /// The fake station of the live tests (the id of `test_station(1)`).
    const STATION: &str = "radio:00000000-0000-4000-8000-000000000001";
    const LIVE: f32 = 0.125;
    /// n1 and n2 are Navidrome tracks, the station is endless.
    const LIVE_TONES: [(&str, f32, u64); 3] = [
        ("n1", NET_A, 2_000),
        (STATION, LIVE, 0),
        ("n2", NET_B, 1_000),
    ];

    /// (track id, kind, source, message) of every error.
    fn error_details(events: &[PlayerEvent]) -> Vec<(String, ErrorKind, Source, String)> {
        events
            .iter()
            .filter_map(|event| match event {
                PlayerEvent::Error(error) => Some((
                    error.track_id.clone(),
                    error.kind,
                    error.source,
                    error.message.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    fn station_offline() -> (String, ErrorKind, Source, String) {
        (
            STATION.to_string(),
            ErrorKind::Offline,
            Source::Radio,
            STATION_OFFLINE_MESSAGE.to_string(),
        )
    }

    fn wait_until(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "{what} never happened");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Plays `queue` from `start` until the station is heard; the events so far.
    fn play_live(
        server: &Arc<NetServer>,
        options: PlayerOptions,
        queue: &[&str],
        start: usize,
        extra: Option<EventSink>,
    ) -> (Harness, Vec<PlayerEvent>) {
        assert_eq!(test_station(1).id, STATION);
        let h = net_harness_with(&LIVE_TONES, server, FAST, options, extra);
        h.player.play(ids(queue), start).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(5), |seen| {
            changes(seen).contains(&STATION.to_string())
        });
        wait_state(&h.player, |s| {
            playing_track(s, STATION) && s.position_ms > 0
        });
        (h, events)
    }

    fn play_station(server: &Arc<NetServer>) -> (Harness, Vec<PlayerEvent>) {
        play_live(server, PlayerOptions::default(), &[STATION], 0, None)
    }

    #[test]
    fn live_has_no_duration_and_no_seek() {
        let server = NetServer::new();
        let (h, _) = play_station(&server);
        let state = h.player.state();
        assert_eq!(state.duration_ms, 0);
        assert!(is_live(state.track.as_ref().unwrap()));
        h.player.seek(5_000).unwrap();
        h.player.seek(0).unwrap();
        let before = h.player.state().position_ms;
        std::thread::sleep(Duration::from_millis(200));
        let later: Vec<PlayerEvent> = h.events.try_iter().collect();
        assert!(errors(&later).is_empty(), "{later:#?}");
        assert!(server.seeks().is_empty(), "{:?}", server.seeks());
        assert_eq!(server.opens(), ids(&[STATION]));
        let state = h.player.state();
        assert!(playing_track(&state, STATION), "{state:#?}");
        assert!(state.position_ms > before, "the time heard goes on");
    }

    #[test]
    fn next_and_prev_do_nothing_on_live() {
        for repeat in [RepeatMode::Off, RepeatMode::All, RepeatMode::One] {
            let server = NetServer::new();
            let (h, _) = play_live(
                &server,
                with_repeat(repeat),
                &["n1", STATION, "n2"],
                1,
                None,
            );
            h.player.next().unwrap();
            h.player.prev().unwrap();
            h.player.next().unwrap();
            h.player.set_shuffle(true).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            let later: Vec<PlayerEvent> = h.events.try_iter().collect();
            assert!(changes(&later).is_empty(), "{repeat:?}: {later:#?}");
            assert!(errors(&later).is_empty(), "{repeat:?}: {later:#?}");
            assert_eq!(server.opens(), ids(&[STATION]), "{repeat:?}");
            let state = h.player.state();
            assert!(playing_track(&state, STATION), "{repeat:?}: {state:#?}");
            // Held (paused) on the station, they do nothing either.
            h.player.toggle().unwrap();
            wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
            h.player.next().unwrap();
            h.player.prev().unwrap();
            std::thread::sleep(Duration::from_millis(100));
            let state = h.player.state();
            assert_eq!(state.status, PlayerStatus::Paused, "{repeat:?}");
            assert_eq!(state.track.unwrap().id, STATION, "{repeat:?}");
            assert_eq!(server.opens(), ids(&[STATION]), "{repeat:?}");
        }
    }

    #[test]
    fn no_preopen_or_crossfade_with_live() {
        for options in [PlayerOptions::default(), crossfade_always(1_000)] {
            let server = NetServer::new();
            let (h, events) = play_live(&server, options.clone(), &["n1", STATION], 0, None);
            assert_eq!(changes(&events), ids(&["n1", STATION]));
            assert!(errors(&events).is_empty(), "{events:#?}");
            let threads = server.open_threads();
            assert_eq!(
                threads
                    .iter()
                    .filter(|(_, thread)| thread == OPENER_NEXT_THREAD)
                    .count(),
                0,
                "{options:?}: nothing was pre-opened: {threads:?}"
            );
            assert_eq!(
                played_of(&h.played, NET_A),
                samples_of(2_000),
                "{options:?}: all of n1 was heard"
            );
            let played = h.played.lock().unwrap().clone();
            assert!(
                played
                    .iter()
                    .all(|s| (*s - NET_A).abs() < 1e-6 || (*s - LIVE).abs() < 1e-6),
                "{options:?}: a hard switch, nothing blended"
            );
        }
    }

    #[test]
    fn pause_closes_and_play_reopens_live() {
        let server = NetServer::new();
        let (h, mut events) = play_station(&server);
        h.player.toggle().unwrap();
        let paused = wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
        assert_eq!(paused.position_ms, 0);
        assert_eq!(paused.track.as_ref().unwrap().id, STATION);
        wait_until("closing the station", || {
            server.drops().contains(&STATION.to_string())
        });
        let heard = played_of(&h.played, LIVE);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            played_of(&h.played, LIVE),
            heard,
            "nothing plays while paused"
        );
        events.extend(h.events.try_iter());
        assert!(
            errors(&events).is_empty(),
            "a pause is no notice: {events:#?}"
        );
        h.player.toggle().unwrap();
        wait_state(&h.player, |s| playing_track(s, STATION));
        assert_eq!(server.opens(), ids(&[STATION, STATION]));
        assert!(server.seeks().is_empty(), "joined live, not at a position");
        std::thread::sleep(Duration::from_millis(100));
        events.extend(h.events.try_iter());
        assert_eq!(changes(&events), ids(&[STATION]), "one play: {events:#?}");
        assert!(errors(&events).is_empty(), "{events:#?}");
    }

    #[test]
    fn pause_while_a_station_loads_holds_it() {
        let server = NetServer::new();
        server.set(|c| c.open_delay = Duration::from_millis(300));
        let h = net_harness(&LIVE_TONES, &server, FAST, PlayerOptions::default());
        h.player.play(ids(&[STATION]), 0).unwrap();
        wait_state(&h.player, |s| s.status == PlayerStatus::Loading);
        h.player.toggle().unwrap();
        wait_until("the open", || !server.opens().is_empty());
        wait_until("closing the station", || {
            server.drops().contains(&STATION.to_string())
        });
        let state = wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
        assert_eq!(state.track.as_ref().unwrap().id, STATION);
        assert_eq!(played_of(&h.played, LIVE), 0);
        h.player.toggle().unwrap();
        wait_state(&h.player, |s| playing_track(s, STATION));
        std::thread::sleep(Duration::from_millis(100));
        let events: Vec<PlayerEvent> = h.events.try_iter().collect();
        assert_eq!(changes(&events), ids(&[STATION]), "{events:#?}");
        assert!(errors(&events).is_empty(), "{events:#?}");
        assert_eq!(server.opens(), ids(&[STATION, STATION]));
    }

    /// Plays the station until its stream is gone for good; the events and the paused state.
    fn play_station_until_offline(
        server: &Arc<NetServer>,
    ) -> (Harness, Vec<PlayerEvent>, PlayerState) {
        let (h, mut events) = play_station(server);
        server.set(|c| c.cut_at = vec![(STATION.into(), 0)]);
        events.extend(collect_until(&h.events, Duration::from_secs(5), |seen| {
            !errors(seen).is_empty()
        }));
        let state = wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
        (h, events, state)
    }

    #[test]
    fn live_offline_pauses_with_one_notice() {
        let server = NetServer::new();
        let (h, events, state) = play_station_until_offline(&server);
        assert_eq!(error_details(&events), vec![station_offline()]);
        assert_eq!(state.track.as_ref().unwrap().id, STATION);
        assert_eq!(state.position_ms, 0);
        assert_eq!(state.queue_index, 0);
        std::thread::sleep(Duration::from_millis(300));
        let later: Vec<PlayerEvent> = h.events.try_iter().collect();
        assert!(errors(&later).is_empty(), "one notice: {later:#?}");
        assert_eq!(server.opens(), ids(&[STATION]), "no reconnect loop");
        assert_eq!(h.player.state().status, PlayerStatus::Paused);
    }

    #[test]
    fn play_after_offline_joins_live() {
        let server = NetServer::new();
        let (h, mut events, _) = play_station_until_offline(&server);
        server.set(|c| c.cut_at.clear());
        h.player.toggle().unwrap();
        wait_state(&h.player, |s| playing_track(s, STATION));
        std::thread::sleep(Duration::from_millis(100));
        events.extend(h.events.try_iter());
        assert_eq!(server.opens(), ids(&[STATION, STATION]));
        assert!(
            server.seeks().is_empty(),
            "reopened live, not at a position"
        );
        assert_eq!(changes(&events), ids(&[STATION]), "{events:#?}");
        assert_eq!(error_details(&events), vec![station_offline()]);
    }

    #[test]
    fn a_live_end_is_held_not_idle() {
        let server = NetServer::new();
        let (h, mut events) = play_station(&server);
        server.set(|c| c.ended = vec![STATION.into()]);
        events.extend(collect_until(&h.events, Duration::from_secs(5), |seen| {
            !errors(seen).is_empty()
        }));
        let state = wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
        assert_eq!(state.track.as_ref().unwrap().id, STATION);
        std::thread::sleep(Duration::from_millis(300));
        events.extend(h.events.try_iter());
        assert_eq!(error_details(&events), vec![station_offline()]);
        assert!(
            !back_to_idle(&events),
            "never idle on a station: {events:#?}"
        );
        assert_eq!(h.player.state().status, PlayerStatus::Paused);
    }

    #[test]
    fn a_failing_station_ends_the_queue_once() {
        let cases: [(&str, NetSetup); 2] = [
            ("open", |c| c.fail_ids = vec![STATION.into()]),
            ("first read", |c| c.broken_at = vec![(STATION.into(), 0)]),
        ];
        for (case, setup) in cases {
            for repeat in [RepeatMode::Off, RepeatMode::All, RepeatMode::One] {
                let server = NetServer::new();
                server.set(setup);
                let h = net_harness(&LIVE_TONES, &server, FAST, with_repeat(repeat));
                h.player.play(ids(&[STATION]), 0).unwrap();
                let mut events = collect_until(&h.events, Duration::from_secs(5), back_to_idle);
                std::thread::sleep(Duration::from_millis(200));
                events.extend(h.events.try_iter());
                let details = error_details(&events);
                assert_eq!(details.len(), 1, "{case} {repeat:?}: {details:?}");
                assert_eq!(details[0].1, ErrorKind::Track, "{case} {repeat:?}");
                assert_eq!(details[0].2, Source::Radio, "{case} {repeat:?}");
                assert_eq!(server.opens(), ids(&[STATION]), "{case} {repeat:?}");
                assert_eq!(h.player.state().status, PlayerStatus::Idle);
            }
        }
    }

    #[test]
    fn navidrome_offline_message_is_unchanged() {
        let server = NetServer::new();
        server.set(|c| c.down = true);
        let h = net_harness(&LIVE_TONES, &server, FAST, PlayerOptions::default());
        h.player.play(ids(&["n1"]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(5), |seen| {
            !errors(seen).is_empty()
        });
        assert_eq!(
            error_details(&events),
            vec![(
                "n1".to_string(),
                ErrorKind::Offline,
                Source::Navidrome,
                OFFLINE_MESSAGE.to_string()
            )]
        );
        let error = events.iter().find_map(|e| match e {
            PlayerEvent::Error(error) => Some(error.clone()),
            _ => None,
        });
        let json = serde_json::to_value(error.unwrap()).unwrap();
        assert_eq!(json["source"], "navidrome");
        assert_eq!(json["kind"], "offline");
    }

    #[test]
    fn radio_plays_are_not_recorded_or_scrobbled() {
        let server = NetServer::new();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let actions = Arc::new(Mutex::new(Vec::<ScrobbleAction>::new()));
        let tracker = Arc::new(Mutex::new(ScrobbleTracker::new()));
        let scrobbles: EventSink = {
            let actions = Arc::clone(&actions);
            Arc::new(move |event| {
                let out = tracker.lock().unwrap().on_event(&event, 1_700_000_000_000);
                actions.lock().unwrap().extend(out);
            })
        };
        let extra = fan_out(vec![spawn_recorder(Arc::clone(&db)), scrobbles]);
        let (h, _) = play_live(
            &server,
            PlayerOptions::default(),
            &[STATION],
            0,
            Some(extra),
        );
        h.player.toggle().unwrap();
        wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
        h.player.toggle().unwrap();
        wait_state(&h.player, |s| playing_track(s, STATION));
        // A track after it is recorded, so the recorder has seen everything before it.
        h.player.play(ids(&["n1"]), 0).unwrap();
        let count = |sql: &str| -> i64 {
            db.0.lock()
                .unwrap()
                .query_row(sql, [], |row| row.get(0))
                .unwrap()
        };
        wait_until("the play of n1", || {
            count("SELECT COUNT(*) FROM track_stats WHERE track_id = 'n1'") == 1
        });
        assert_eq!(count("SELECT COUNT(*) FROM track_stats"), 1);
        assert!(actions.lock().unwrap().is_empty());
    }

    #[test]
    fn live_gain_is_unity() {
        let mut station = test_station(1).as_track();
        station.replay_gain_track_db = Some(-9.0);
        station.replay_gain_album_db = Some(-6.0);
        station.replay_gain_track_peak = Some(0.5);
        for mode in [ReplayGain::Off, ReplayGain::Track, ReplayGain::Album] {
            assert_eq!(track_gain(&station, mode), 1.0, "{mode:?}");
        }
        // The same tags on a track still apply.
        let mut track = station.clone();
        track.source = Source::Navidrome;
        track.duration_ms = 1_000;
        assert!(track_gain(&track, ReplayGain::Track) < 1.0);
    }

    #[test]
    fn is_live_needs_radio_and_no_duration() {
        let station = test_station(1).as_track();
        assert!(is_live(&station));
        let mut timed = station.clone();
        timed.duration_ms = 1_000;
        assert!(!is_live(&timed));
        let mut navidrome = station;
        navidrome.source = Source::Navidrome;
        assert!(!is_live(&navidrome));
    }

    #[test]
    fn next_acts_after_a_jump_away_from_a_station() {
        let server = NetServer::new();
        let (h, _) = play_live(
            &server,
            PlayerOptions::default(),
            &[STATION, "n1", "n2"],
            0,
            None,
        );
        server.set(|c| c.open_delay = Duration::from_millis(300));
        h.player.jump(1).unwrap();
        // n1 is still loading; the station is still shown.
        wait_state(&h.player, |s| s.status == PlayerStatus::Loading);
        h.player.next().unwrap();
        let events = collect_until(&h.events, Duration::from_secs(5), |seen| {
            !changes(seen).is_empty()
        });
        assert_eq!(changes(&events), ids(&["n2"]), "{events:#?}");
        let state = wait_state(&h.player, |s| playing_track(s, "n2"));
        assert_eq!(state.queue_index, 2);
    }

    #[test]
    fn next_while_a_station_loads_is_ignored() {
        let server = NetServer::new();
        let tones = [
            ("n1", NET_A, SILENCE_MS),
            (STATION, LIVE, 0),
            ("n2", NET_B, 1_000),
        ];
        let h = net_harness(&tones, &server, 1.0, PlayerOptions::default());
        h.player.play(ids(&["n1", STATION, "n2"]), 0).unwrap();
        wait_state(&h.player, |s| playing_track(s, "n1"));
        server.set(|c| c.open_delay = Duration::from_millis(300));
        h.player.jump(1).unwrap();
        wait_state(&h.player, |s| s.status == PlayerStatus::Loading);
        h.player.next().unwrap();
        h.player.prev().unwrap();
        let state = wait_state(&h.player, |s| playing_track(s, STATION));
        assert_eq!(state.queue_index, 1);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(server.opens(), ids(&["n1", STATION]));
        let events: Vec<PlayerEvent> = h.events.try_iter().collect();
        assert_eq!(changes(&events), ids(&["n1", STATION]), "{events:#?}");
    }

    #[test]
    fn an_unknown_station_with_repeat_all_reports_once_and_goes_idle() {
        let server = NetServer::new();
        let unknown = "radio:00000000-0000-4000-8000-0000000000ff";
        let h = net_harness(&LIVE_TONES, &server, FAST, with_repeat(RepeatMode::All));
        h.player.play(ids(&[unknown]), 0).unwrap();
        let mut events = collect_until(&h.events, Duration::from_secs(5), back_to_idle);
        std::thread::sleep(Duration::from_millis(200));
        events.extend(h.events.try_iter());
        let details = error_details(&events);
        assert_eq!(details.len(), 1, "{details:?}");
        assert_eq!(details[0].0, unknown);
        assert_eq!(details[0].1, ErrorKind::Track);
        assert_eq!(details[0].2, Source::Radio);
        assert!(server.opens().is_empty());
        assert_eq!(h.player.state().status, PlayerStatus::Idle);
    }

    #[test]
    fn repeat_all_goes_round_past_a_failing_station() {
        let server = NetServer::new();
        server.set(|c| c.fail_ids = vec![STATION.into()]);
        let tones = [("n1", NET_A, 500), (STATION, LIVE, 0)];
        let h = net_harness(&tones, &server, FAST, with_repeat(RepeatMode::All));
        h.player.play(ids(&["n1", STATION]), 0).unwrap();
        let events = collect_until(&h.events, Duration::from_secs(10), |seen| {
            changes(seen).len() == 2
        });
        assert_eq!(changes(&events), ids(&["n1", "n1"]), "{events:#?}");
        let details = error_details(&events);
        assert_eq!(details.len(), 1, "{details:?}");
        assert_eq!(details[0].0, STATION);
        assert_eq!(details[0].1, ErrorKind::Track);
        assert!(!back_to_idle(&events), "not idle: {events:#?}");
    }

    /// Ten seconds of silence per track, opened after `delay`.
    fn silence_harness(delay: Duration) -> Harness {
        let mut track = fixture_track("long1", "tagged.flac");
        track.duration_ms = SILENCE_MS;
        let opener: Opener = Arc::new(move |_track: &Track, _spec: StreamSpec| {
            std::thread::sleep(delay);
            Ok(Box::new(Silence {
                frames_left: SILENCE_MS * 44_100 / 1000,
            }) as Box<dyn AudioSource>)
        });
        harness(vec![track], FAST, Some(opener))
    }

    fn statuses(events: &[PlayerEvent]) -> Vec<PlayerStatus> {
        events
            .iter()
            .filter_map(|event| match event {
                PlayerEvent::State(state) => Some(state.status),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn engine_pause_pauses_only_when_playing() {
        let h = silence_harness(Duration::ZERO);
        // Idle: nothing to pause, nothing starts.
        h.player.pause().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(h.player.state().status, PlayerStatus::Idle);
        let idle: Vec<PlayerEvent> = h.events.try_iter().collect();
        assert!(
            statuses(&idle).iter().all(|s| *s == PlayerStatus::Idle),
            "{idle:#?}"
        );

        h.player.play(ids(&["long1"]), 0).unwrap();
        wait_state(&h.player, |s| {
            s.status == PlayerStatus::Playing && s.position_ms > 0
        });
        h.player.pause().unwrap();
        let paused = wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
        // Paused already: a second pause is no toggle.
        h.player.handle().pause().unwrap();
        std::thread::sleep(Duration::from_millis(150));
        let state = h.player.state();
        assert_eq!(state.status, PlayerStatus::Paused);
        assert_eq!(state.position_ms, paused.position_ms, "nothing played on");
        let later: Vec<PlayerEvent> = h.events.try_iter().collect();
        assert_eq!(changes(&later), ids(&["long1"]), "one play: {later:#?}");

        h.player.toggle().unwrap();
        wait_state(&h.player, |s| s.status == PlayerStatus::Playing);
    }

    #[test]
    fn engine_pause_while_loading_starts_paused() {
        let h = silence_harness(Duration::from_millis(300));
        h.player.play(ids(&["long1"]), 0).unwrap();
        wait_state(&h.player, |s| s.status == PlayerStatus::Loading);
        h.player.pause().unwrap();
        // A second pause while loading keeps it paused (toggle would flip it back).
        h.player.pause().unwrap();
        let state = wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
        assert_eq!(state.track.unwrap().id, "long1");
        std::thread::sleep(Duration::from_millis(150));
        let state = h.player.state();
        assert_eq!(state.status, PlayerStatus::Paused);
        assert_eq!(state.position_ms, 0);
        let events: Vec<PlayerEvent> = h.events.try_iter().collect();
        assert!(
            !statuses(&events).contains(&PlayerStatus::Playing),
            "{events:#?}"
        );
    }

    #[test]
    fn pause_on_a_station_holds_it() {
        let server = NetServer::new();
        let (h, mut events) = play_station(&server);
        h.player.handle().pause().unwrap();
        let paused = wait_state(&h.player, |s| s.status == PlayerStatus::Paused);
        assert_eq!(paused.position_ms, 0);
        assert_eq!(paused.track.as_ref().unwrap().id, STATION);
        wait_until("closing the station", || {
            server.drops().contains(&STATION.to_string())
        });
        // Held: pausing again neither reopens nor resumes it.
        h.player.pause().unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(h.player.state().status, PlayerStatus::Paused);
        assert_eq!(server.opens(), ids(&[STATION]));
        events.extend(h.events.try_iter());
        assert!(errors(&events).is_empty(), "no notice: {events:#?}");
        // Play joins it live again, as the same play.
        h.player.toggle().unwrap();
        wait_state(&h.player, |s| playing_track(s, STATION));
        std::thread::sleep(Duration::from_millis(100));
        events.extend(h.events.try_iter());
        assert_eq!(changes(&events), ids(&[STATION]), "{events:#?}");
        assert_eq!(server.opens(), ids(&[STATION, STATION]));
    }
}
