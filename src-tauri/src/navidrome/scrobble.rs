//! Scrobbling. A Navidrome track that becomes audible is announced as now playing
//! (`submission=false`, best effort); once it has been heard for min(half its length, 4 min)
//! and is at least 30 s long, the play is submitted (`submission=true`, `time` = when the play
//! started). A submission the server did not take waits in `navidrome_scrobble_queue`, which
//! survives a restart, and every cycle sends it last ([`flush_queue`]).
//!
//! [`ScrobbleTracker`] turns player events into these actions. Listened time is what the
//! player's position actually moved while playing: paused time, seeks and a position that
//! jumps at a gapless hand-over are not counted. Each step is capped by the wall-clock time
//! between two state events, and the sum by the play's whole playing wall-clock time. Every
//! `TrackChanged` of a Navidrome track is a new play, also for the same id (a repeat, a
//! replay); a resume after an offline pause sends only states, so it stays the same play.
//!
//! Limits, by design:
//! - A submit while no client exists (signed out) is dropped, not queued: nothing of the
//!   server can play without a client.
//! - A submission whose response is lost after the server took it is queued and sent again,
//!   so it can count twice.
//! - Connecting (to another or the same account) drops the play that is in flight: its
//!   submit belongs to the epoch it was announced under.
//! - A queued submission the server answers with an error code (`Server{code}`), `NotFound`
//!   or `NotAuthorized` is dropped, not retried (as for stars).

use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;

use rusqlite::params;

use super::client::{NavidromeError, SubsonicClient};
use super::stars::ID_PREFIX;
use super::sync::SyncError;
use super::{answers, now_ms, Core, NavidromeService};
use crate::db::Db;
use crate::library::scan::lock;
use crate::player::{EventSink, PlayerEvent, PlayerState, PlayerStatus};

/// Shorter tracks are never submitted.
pub const MIN_TRACK_MS: u64 = 30_000;
/// A track counts as played after half its length, or after this, whichever comes first.
pub const MAX_THRESHOLD_MS: u64 = 240_000;
/// A forward position step above this between two state events is a seek, not listening.
pub const MAX_STEP_MS: u64 = 2_000;
/// What a position step may exceed the wall-clock time between its two events by (the output
/// position moves in buffer-sized jumps).
const STEP_SLACK_MS: u64 = 250;
/// Queued submissions one cycle sends at most.
pub const FLUSH_BATCH: u32 = 50;

/// What the scrobbler sends. Ids are raw Subsonic ids (without `navidrome:`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScrobbleAction {
    NowPlaying { id: String },
    Submit { id: String, played_at: i64 },
}

/// The play of one Navidrome track.
struct Play {
    /// The track id (`navidrome:<id>`).
    track_id: String,
    duration_ms: u64,
    started_at: i64,
    listened_ms: u64,
    /// Wall-clock time of the play's playing states, the bound of `listened_ms`.
    playing_ms: u64,
    /// The last position seen and when, the base of the next step.
    last: Option<(u64, i64)>,
    submitted: bool,
}

impl Play {
    fn new(track_id: String, duration_ms: u64, now_ms: i64) -> Play {
        Play {
            track_id,
            duration_ms,
            started_at: now_ms,
            listened_ms: 0,
            playing_ms: 0,
            // A new play is heard from its start.
            last: Some((0, now_ms)),
            submitted: false,
        }
    }

    fn raw_id(&self) -> String {
        self.track_id
            .strip_prefix(ID_PREFIX)
            .unwrap_or(&self.track_id)
            .to_owned()
    }

    fn threshold_ms(&self) -> u64 {
        (self.duration_ms / 2).min(MAX_THRESHOLD_MS)
    }
}

/// Turns player events into now-playing and submit actions. Pure: the caller passes the time.
#[derive(Default)]
pub struct ScrobbleTracker {
    play: Option<Play>,
}

/// The raw Subsonic id of a Navidrome track id.
fn navidrome_id(track_id: &str) -> Option<&str> {
    track_id
        .strip_prefix(ID_PREFIX)
        .filter(|raw| !raw.is_empty())
}

impl ScrobbleTracker {
    pub fn new() -> ScrobbleTracker {
        ScrobbleTracker::default()
    }

    pub fn on_event(&mut self, event: &PlayerEvent, now_ms: i64) -> Vec<ScrobbleAction> {
        match event {
            PlayerEvent::TrackChanged(track) => {
                let Some(raw) = navidrome_id(&track.id) else {
                    // A local track: nothing of it is sent, and it ends the Navidrome play.
                    self.play = None;
                    return Vec::new();
                };
                // Always a new play: the same id again is a repeat or a replay.
                self.play = Some(Play::new(track.id.clone(), track.duration_ms, now_ms));
                vec![ScrobbleAction::NowPlaying { id: raw.to_owned() }]
            }
            PlayerEvent::State(state) => self.on_state(state, now_ms),
            PlayerEvent::Error(_) | PlayerEvent::Level(_) => Vec::new(),
        }
    }

    fn on_state(&mut self, state: &PlayerState, now_ms: i64) -> Vec<ScrobbleAction> {
        let Some(play) = &mut self.play else {
            return Vec::new();
        };
        // Another track shown (one that failed to open, a held entry) is not this play.
        if state.track.as_ref().map(|t| t.id.as_str()) != Some(play.track_id.as_str()) {
            return Vec::new();
        }
        if play.duration_ms == 0 {
            play.duration_ms = state.duration_ms;
        }
        let position = state.position_ms;
        if state.status == PlayerStatus::Playing {
            if let Some((last_position, last_at)) = play.last {
                let step = position.saturating_sub(last_position);
                let elapsed = u64::try_from(now_ms.saturating_sub(last_at)).unwrap_or(0);
                // A long gap is a pause (or a hold) that just ended, not playing time.
                play.playing_ms += elapsed.min(MAX_STEP_MS);
                if step <= MAX_STEP_MS {
                    play.listened_ms = (play.listened_ms + step.min(elapsed + STEP_SLACK_MS))
                        .min(play.playing_ms + STEP_SLACK_MS);
                }
            }
        }
        play.last = Some((position, now_ms));
        if !play.submitted
            && play.duration_ms >= MIN_TRACK_MS
            && play.listened_ms >= play.threshold_ms()
        {
            play.submitted = true;
            return vec![ScrobbleAction::Submit {
                id: play.raw_id(),
                played_at: play.started_at,
            }];
        }
        Vec::new()
    }
}

/// Errors that say nothing about the scrobble: the account or the connection is wrong. The
/// flush stops and keeps the queue.
fn keeps_queue(err: &NavidromeError) -> bool {
    err.is_offline()
        || matches!(
            err,
            NavidromeError::InvalidUrl(_)
                | NavidromeError::WrongCredentials
                | NavidromeError::TokenAuthUnsupported
                | NavidromeError::Tls(_)
                | NavidromeError::NotSubsonic(_)
        )
}

/// Errors that end the cycle: offline, or the credentials, TLS or the address are wrong.
fn ends_cycle(err: &NavidromeError) -> bool {
    err.is_offline()
        || matches!(
            err,
            NavidromeError::InvalidUrl(_)
                | NavidromeError::WrongCredentials
                | NavidromeError::TokenAuthUnsupported
                | NavidromeError::Tls(_)
        )
}

/// What one queue flush did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct QueueFlush {
    /// Submissions the server took.
    pub sent: u32,
    /// Submissions the server refused (the track is gone, not allowed); they are not retried.
    pub dropped: u32,
    /// The error that stopped the flush; the rest of the queue stays.
    pub stopped: Option<NavidromeError>,
}

/// One queued submission.
struct Queued {
    row: i64,
    track_id: String,
    played_at: i64,
}

/// Sends the queued submissions, oldest first, at most [`FLUSH_BATCH`]. Stops once
/// `cancelled` says so (the account changed or the app quits); `cancelled` is checked under
/// the database lock before every write, and network calls run outside that lock.
pub fn flush_queue(
    db: &Db,
    client: &SubsonicClient,
    cancelled: &dyn Fn() -> bool,
) -> anyhow::Result<QueueFlush> {
    let mut outcome = QueueFlush::default();
    let queue = {
        let conn = lock(db)?;
        if cancelled() {
            return Ok(outcome);
        }
        let mut stmt = conn.prepare(
            "SELECT id, track_id, played_at FROM navidrome_scrobble_queue
             ORDER BY played_at, id LIMIT ?1",
        )?;
        let rows = stmt.query_map([FLUSH_BATCH], |row| {
            Ok(Queued {
                row: row.get(0)?,
                track_id: row.get(1)?,
                played_at: row.get(2)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for item in queue {
        if cancelled() {
            break;
        }
        let result = match navidrome_id(&item.track_id) {
            Some(raw) => client.scrobble(raw, item.played_at, true),
            None => Err(NavidromeError::NotFound),
        };
        if let Err(err) = &result {
            if keeps_queue(err) {
                outcome.stopped = result.err();
                break;
            }
        }
        let conn = lock(db)?;
        if cancelled() {
            break;
        }
        conn.execute(
            "DELETE FROM navidrome_scrobble_queue WHERE id = ?1",
            [item.row],
        )?;
        match result {
            Ok(()) => outcome.sent += 1,
            Err(err) => {
                outcome.dropped += 1;
                eprintln!("Navidrome dropped a scrobble: {}", err.code());
            }
        }
    }
    Ok(outcome)
}

impl Core {
    /// The cycle's last step: sends the queued submissions. Offline and account errors end
    /// the cycle (the queue stays); any other answer only skips the rest of the step.
    pub(super) fn flush_scrobbles(
        &self,
        client: &SubsonicClient,
        epoch: u64,
    ) -> Result<(), SyncError> {
        let outcome = flush_queue(&self.db, client, &|| !self.is_current(epoch))?;
        if !self.is_current(epoch) {
            return Err(SyncError::Cancelled);
        }
        match outcome.stopped {
            Some(err) if ends_cycle(&err) => Err(SyncError::Api(err)),
            Some(err) => {
                eprintln!("Navidrome scrobble flush: {}", err.code());
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// The signed-in account's client, its epoch and whether the server answers.
    /// `None` once the service is shut down.
    fn scrobble_target(&self) -> Option<(Arc<SubsonicClient>, u64, bool)> {
        let inner = self.inner();
        if inner.shutdown {
            return None;
        }
        let client = inner.client.clone()?;
        Some((client, inner.epoch, answers(&inner.status)))
    }

    /// Queues a submission for the account of `epoch`. Writes nothing (false) once another
    /// account is signed in: its queue must never carry this one's plays.
    fn queue_scrobble(&self, epoch: u64, track_id: &str, played_at: i64) -> anyhow::Result<bool> {
        let conn = lock(&self.db)?;
        if !self.is_current(epoch) {
            return Ok(false);
        }
        conn.execute(
            "INSERT INTO navidrome_scrobble_queue (track_id, played_at) VALUES (?1, ?2)",
            params![track_id, played_at],
        )?;
        Ok(true)
    }
}

/// The scrobbler thread: runs the tracker and talks to the server.
struct Scrobbler {
    service: Arc<NavidromeService>,
    tracker: ScrobbleTracker,
    /// The raw id of the last now-playing and the account epoch it was announced under. A
    /// submit counts only for that account.
    announced: Option<(String, Option<u64>)>,
}

impl Scrobbler {
    fn handle(&mut self, event: &PlayerEvent, at_ms: i64) {
        for action in self.tracker.on_event(event, at_ms) {
            match action {
                ScrobbleAction::NowPlaying { id } => self.now_playing(id, at_ms),
                ScrobbleAction::Submit { id, played_at } => self.submit(&id, played_at),
            }
        }
    }

    /// Best effort: a failure is not retried.
    fn now_playing(&mut self, id: String, at_ms: i64) {
        let target = self.service.core.scrobble_target();
        self.announced = Some((id.clone(), target.as_ref().map(|(_, epoch, _)| *epoch)));
        if let Some((client, _, true)) = target {
            if let Err(err) = client.scrobble(&id, at_ms, false) {
                eprintln!("Navidrome now playing: {}", err.code());
            }
        }
    }

    /// Sends the play now, or queues it for the next cycle. Dropped when the account changed
    /// since the track was announced, or none is signed in.
    fn submit(&mut self, id: &str, played_at: i64) {
        let core = &self.service.core;
        let Some((client, epoch, reachable)) = core.scrobble_target() else {
            return;
        };
        let same_account = matches!(
            &self.announced,
            Some((announced, Some(at))) if announced == id && *at == epoch
        );
        if !same_account {
            return;
        }
        if reachable {
            match client.scrobble(id, played_at, true) {
                Ok(()) => return,
                Err(err) => eprintln!("Navidrome scrobble queued: {}", err.code()),
            }
        }
        let track_id = format!("{ID_PREFIX}{id}");
        if let Err(err) = core.queue_scrobble(epoch, &track_id, played_at) {
            eprintln!("queueing a Navidrome scrobble failed: {err:#}");
        }
    }
}

/// A player sink that scrobbles Navidrome tracks. The tracker and every server call run on a
/// thread of its own, so the player thread never waits on the network; each event carries
/// the time it was sent.
pub fn spawn_scrobbler(service: Arc<NavidromeService>) -> EventSink {
    spawn_with_clock(service, Arc::new(now_ms)).0
}

/// [`spawn_scrobbler`] with a clock, also handing out the thread (which ends once every clone
/// of the sink is dropped).
pub(crate) fn spawn_with_clock(
    service: Arc<NavidromeService>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
) -> (EventSink, Option<JoinHandle<()>>) {
    let (sender, receiver) = mpsc::channel::<(PlayerEvent, i64)>();
    let thread = std::thread::Builder::new()
        .name("prism-scrobbler".into())
        .spawn(move || {
            let mut scrobbler = Scrobbler {
                service,
                tracker: ScrobbleTracker::new(),
                announced: None,
            };
            for (event, at_ms) in receiver {
                scrobbler.handle(&event, at_ms);
            }
        })
        .map_err(|err| eprintln!("starting the Navidrome scrobbler failed: {err}"))
        .ok();
    let sink: EventSink = Arc::new(move |event| {
        if matches!(event, PlayerEvent::State(_) | PlayerEvent::TrackChanged(_)) {
            let _ = sender.send((event, clock()));
        }
    });
    (sink, thread)
}

#[cfg(test)]
mod tests;
