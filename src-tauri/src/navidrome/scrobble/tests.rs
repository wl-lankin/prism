//! The tracker (pure, with a hand-run clock), the queue flush against the fake server, and the
//! scrobbler sink on a running service.

use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::*;
use crate::library::model::{Source, Track};
use crate::navidrome::keychain::{CredentialStore, MemoryStore};
use crate::navidrome::sync::{ensure_server, server_key};
use crate::navidrome::testserver::{FakeLibrary, FakeServer, Fault, FAKE_PASSWORD, FAKE_USER};
use crate::navidrome::{LinkState, ServiceOptions};
use crate::secret::Secret;
use crate::settings::{RepeatMode, SettingsStore};

const WAIT: Duration = Duration::from_secs(15);
const START: i64 = 1_700_000_000_000;

fn track(id: &str, duration_ms: u64) -> Track {
    Track {
        id: id.into(),
        source: if id.starts_with(ID_PREFIX) {
            Source::Navidrome
        } else {
            Source::Local
        },
        title: "Demo Song".into(),
        artist: "Demo Artist".into(),
        album_artist: "Demo Artist".into(),
        album: "Demo Album".into(),
        album_id: "navidrome:al-0".into(),
        track_no: None,
        disc_no: None,
        duration_ms,
        year: None,
        genre: None,
        format: "mp3".into(),
        sample_rate: None,
        bit_depth: None,
        bitrate_kbps: None,
        cover_id: None,
        path: None,
        replay_gain_track_db: None,
        replay_gain_album_db: None,
        replay_gain_track_peak: None,
        replay_gain_album_peak: None,
    }
}

fn state(track: &Track, status: PlayerStatus, position_ms: u64) -> PlayerState {
    PlayerState {
        status,
        track: Some(track.clone()),
        position_ms,
        duration_ms: track.duration_ms,
        volume: 1.0,
        queue_index: 0,
        queue_length: 1,
        queue_version: 1,
        flow: None,
        repeat: RepeatMode::Off,
        shuffle: false,
    }
}

fn now_playing(id: &str) -> ScrobbleAction {
    ScrobbleAction::NowPlaying { id: id.into() }
}

fn submit(id: &str, played_at: i64) -> ScrobbleAction {
    ScrobbleAction::Submit {
        id: id.into(),
        played_at,
    }
}

/// A tracker with a clock the test moves.
struct Run {
    tracker: ScrobbleTracker,
    now: i64,
}

impl Run {
    fn new() -> Run {
        Run {
            tracker: ScrobbleTracker::new(),
            now: START,
        }
    }

    fn wait(&mut self, ms: u64) {
        self.now += ms as i64;
    }

    fn change(&mut self, track: &Track) -> Vec<ScrobbleAction> {
        self.tracker
            .on_event(&PlayerEvent::TrackChanged(track.clone()), self.now)
    }

    fn state(&mut self, track: &Track, status: PlayerStatus, at: u64) -> Vec<ScrobbleAction> {
        self.tracker
            .on_event(&PlayerEvent::State(state(track, status, at)), self.now)
    }

    /// Plays from `from` to `to` in real time, a state every 250 ms; returns every action.
    fn play(&mut self, track: &Track, from: u64, to: u64) -> Vec<ScrobbleAction> {
        let mut out = Vec::new();
        let mut position = from;
        while position < to {
            let step = 250.min(to - position);
            position += step;
            self.wait(step);
            out.extend(self.state(track, PlayerStatus::Playing, position));
        }
        out
    }
}

#[test]
fn now_playing_on_track_change() {
    let mut run = Run::new();
    let song = track("navidrome:tr-1", 180_000);
    assert_eq!(run.change(&song), [now_playing("tr-1")]);
    assert!(run.play(&song, 0, 10_000).is_empty());
}

#[test]
fn submits_at_half_for_short_tracks() {
    let mut run = Run::new();
    let song = track("navidrome:tr-1", 60_000);
    run.change(&song);
    assert!(run.play(&song, 0, 29_750).is_empty());
    assert_eq!(run.play(&song, 29_750, 30_000), [submit("tr-1", START)]);
}

#[test]
fn submits_at_four_minutes_for_long_tracks() {
    let mut run = Run::new();
    let song = track("navidrome:tr-1", 600_000);
    run.change(&song);
    assert!(run.play(&song, 0, MAX_THRESHOLD_MS - 250).is_empty());
    assert_eq!(
        run.play(&song, MAX_THRESHOLD_MS - 250, MAX_THRESHOLD_MS),
        [submit("tr-1", START)]
    );
}

#[test]
fn seeking_forward_does_not_count() {
    let mut run = Run::new();
    let song = track("navidrome:tr-1", 60_000);
    run.change(&song);
    run.play(&song, 0, 10_000);
    // A jump to the end is a seek, not 50 s of listening.
    run.wait(250);
    assert!(run.state(&song, PlayerStatus::Playing, 55_000).is_empty());
    // Small seeks in quick succession count only the time that passed.
    let mut position = 55_000;
    let mut out = Vec::new();
    for _ in 0..20 {
        run.wait(50);
        position = (position + 1_900).min(60_000);
        out.extend(run.state(&song, PlayerStatus::Playing, position));
    }
    assert!(out.is_empty(), "{out:?}");
    // Back to the start: 10 s, the capped seeks and about 14 s more reach the half.
    run.wait(250);
    run.state(&song, PlayerStatus::Playing, 0);
    let out = run.play(&song, 0, 30_000);
    assert_eq!(out, [submit("tr-1", START)]);
}

#[test]
fn pause_does_not_count() {
    let mut run = Run::new();
    let song = track("navidrome:tr-1", 60_000);
    run.change(&song);
    run.play(&song, 0, 20_000);
    // Paused for ten minutes: states keep coming, the position stands.
    for _ in 0..10 {
        run.wait(60_000);
        assert!(run.state(&song, PlayerStatus::Paused, 20_000).is_empty());
    }
    assert!(run.play(&song, 20_000, 29_750).is_empty());
    assert_eq!(run.play(&song, 29_750, 30_000), [submit("tr-1", START)]);
}

#[test]
fn an_offline_pause_does_not_count_and_the_resume_is_the_same_play() {
    let mut run = Run::new();
    let song = track("navidrome:tr-1", 60_000);
    run.change(&song);
    assert!(run.play(&song, 0, 20_000).is_empty());
    // Held offline at 20 s for five minutes: paused states at the held position. The resume
    // sends no `TrackChanged`, only states from there on.
    for _ in 0..5 {
        run.wait(60_000);
        assert!(run.state(&song, PlayerStatus::Paused, 20_000).is_empty());
    }
    assert!(run.play(&song, 20_000, 29_750).is_empty());
    let mut out = run.play(&song, 29_750, 30_000);
    out.extend(run.play(&song, 30_000, 60_000));
    assert_eq!(out, [submit("tr-1", START)], "one play, its original start");
}

#[test]
fn a_held_at_open_track_is_one_play_once_it_plays() {
    let mut run = Run::new();
    let before = track("navidrome:tr-1", 60_000);
    let held = track("navidrome:tr-2", 60_000);
    run.change(&before);
    run.play(&before, 0, 5_000);
    // tr-2 is held at its open: shown paused at 0, nothing audible, no `TrackChanged`.
    for _ in 0..3 {
        run.wait(10_000);
        assert!(run.state(&held, PlayerStatus::Paused, 0).is_empty());
    }
    // Resumed: it becomes audible now, once.
    run.wait(1_000);
    let started = run.now;
    assert_eq!(run.change(&held), [now_playing("tr-2")]);
    let out = run.play(&held, 0, 60_000);
    assert_eq!(out, [submit("tr-2", started)]);
}

#[test]
fn the_same_id_is_a_new_play_even_away_from_the_start() {
    let mut run = Run::new();
    let song = track("navidrome:tr-1", 60_000);
    run.change(&song);
    assert_eq!(run.play(&song, 0, 30_000), [submit("tr-1", START)]);
    run.wait(1_000);
    let second = run.now;
    assert_eq!(run.change(&song), [now_playing("tr-1")]);
    // The first state shows 10 s: the jump is not listening, but it is a new play.
    run.wait(250);
    assert!(run.state(&song, PlayerStatus::Playing, 10_000).is_empty());
    assert!(run.play(&song, 10_000, 39_750).is_empty());
    assert_eq!(run.play(&song, 39_750, 40_000), [submit("tr-1", second)]);
}

#[test]
fn a_burst_of_states_cannot_outrun_the_clock() {
    let mut run = Run::new();
    let song = track("navidrome:tr-1", 60_000);
    run.change(&song);
    assert!(run.play(&song, 0, 28_000).is_empty());
    // 20 states 50 ms apart, each 250 ms further: 5 s of position in 1 s.
    let mut position = 28_000;
    let mut out = Vec::new();
    for _ in 0..20 {
        run.wait(50);
        position += 250;
        out.extend(run.state(&song, PlayerStatus::Playing, position));
    }
    assert!(out.is_empty(), "{out:?}");
    // Real time again: the half is reached about 0.75 s later, not before.
    assert!(run.play(&song, position, position + 500).is_empty());
    assert_eq!(
        run.play(&song, position + 500, position + 1_000),
        [submit("tr-1", START)]
    );
}

#[test]
fn under_30_seconds_never_submits() {
    let mut run = Run::new();
    let song = track("navidrome:tr-1", MIN_TRACK_MS - 1_000);
    run.change(&song);
    assert!(run.play(&song, 0, MIN_TRACK_MS - 1_000).is_empty());
}

#[test]
fn local_tracks_are_ignored() {
    let mut run = Run::new();
    let local = track("local-1", 60_000);
    assert!(run.change(&local).is_empty());
    assert!(run.play(&local, 0, 60_000).is_empty());
    // A local track also ends a Navidrome play.
    let song = track("navidrome:tr-1", 60_000);
    run.change(&song);
    run.play(&song, 0, 20_000);
    assert!(run.change(&local).is_empty());
    assert!(run.play(&song, 20_000, 40_000).is_empty());
}

#[test]
fn radio_stations_are_never_scrobbled() {
    let mut run = Run::new();
    let station = crate::radio::model::tests::station(1).as_track();
    assert!(run.change(&station).is_empty());
    assert!(run.play(&station, 0, 600_000).is_empty());
    // A station also ends a Navidrome play, like a local track.
    let song = track("navidrome:tr-1", 60_000);
    run.change(&song);
    run.play(&song, 0, 20_000);
    assert!(run.change(&station).is_empty());
    assert!(run.play(&song, 20_000, 40_000).is_empty());
}

#[test]
fn submits_once_per_play() {
    let mut run = Run::new();
    let song = track("navidrome:tr-1", 60_000);
    run.change(&song);
    assert_eq!(run.play(&song, 0, 60_000), [submit("tr-1", START)]);
    // Back to the start within the same play.
    run.wait(250);
    run.state(&song, PlayerStatus::Playing, 0);
    assert!(run.play(&song, 0, 60_000).is_empty());
}

#[test]
fn replaying_the_track_is_a_new_play() {
    let mut run = Run::new();
    let song = track("navidrome:tr-1", 60_000);
    run.change(&song);
    assert_eq!(run.play(&song, 0, 40_000), [submit("tr-1", START)]);
    run.wait(5_000);
    let second = run.now;
    assert_eq!(run.change(&song), [now_playing("tr-1")]);
    run.wait(100);
    assert!(run.state(&song, PlayerStatus::Playing, 100).is_empty());
    assert_eq!(run.play(&song, 100, 30_000), [submit("tr-1", second)]);
}

#[test]
fn repeat_one_loops_are_new_plays() {
    let mut run = Run::new();
    let song = track("navidrome:tr-1", 40_000);
    run.change(&song);
    for _ in 0..3 {
        let started = run.now;
        let out = run.play(&song, 0, 39_000);
        assert_eq!(out, [submit("tr-1", started)]);
        // The gapless hand-over: the state stands at the end, then the loop is audible.
        run.wait(250);
        run.state(&song, PlayerStatus::Playing, 40_000);
        run.wait(750);
        assert_eq!(run.change(&song), [now_playing("tr-1")]);
    }
}

#[test]
fn the_gapless_hand_over_attributes_time_to_each_track() {
    let mut run = Run::new();
    let first = track("navidrome:tr-1", 60_000);
    let second = track("navidrome:tr-2", 60_000);
    run.change(&first);
    // 28 s of the first track, then its position stands at the end during the hand-over.
    assert!(run.play(&first, 0, 28_000).is_empty());
    run.wait(250);
    assert!(run.state(&first, PlayerStatus::Playing, 60_000).is_empty());
    run.wait(2_000);
    assert!(run.state(&first, PlayerStatus::Playing, 60_000).is_empty());
    let started = run.now;
    assert_eq!(run.change(&second), [now_playing("tr-2")]);
    // Late states of the first track change nothing.
    assert!(run.state(&first, PlayerStatus::Playing, 60_000).is_empty());
    assert!(run.play(&second, 0, 29_750).is_empty());
    assert_eq!(run.play(&second, 29_750, 30_000), [submit("tr-2", started)]);
}

#[test]
fn unknown_duration_is_taken_from_the_state() {
    let mut run = Run::new();
    let mut song = track("navidrome:tr-1", 0);
    run.change(&song);
    song.duration_ms = 60_000;
    assert_eq!(run.play(&song, 0, 30_000), [submit("tr-1", START)]);
}

// The queue flush against the fake server.

fn server() -> FakeServer {
    FakeServer::start(FakeLibrary::synthetic(2, 3))
}

fn client(server: &FakeServer) -> SubsonicClient {
    SubsonicClient::new(server.account()).unwrap()
}

fn enqueue(db: &Db, track_id: &str, played_at: i64) {
    lock(db)
        .unwrap()
        .execute(
            "INSERT INTO navidrome_scrobble_queue (track_id, played_at) VALUES (?1, ?2)",
            params![track_id, played_at],
        )
        .unwrap();
}

fn queued(db: &Db) -> Vec<(String, i64)> {
    let conn = lock(db).unwrap();
    let mut stmt = conn
        .prepare("SELECT track_id, played_at FROM navidrome_scrobble_queue ORDER BY id")
        .unwrap();
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap();
    rows.collect::<rusqlite::Result<_>>().unwrap()
}

fn submissions(server: &FakeServer) -> Vec<(String, bool, Option<i64>)> {
    server.scrobbles().into_iter().filter(|s| s.1).collect()
}

#[test]
fn flush_sends_oldest_first_and_clears_the_queue() {
    let server = server();
    let db = Db::open_in_memory().unwrap();
    enqueue(&db, "navidrome:tr-2", 2_000);
    enqueue(&db, "navidrome:tr-1", 1_000);
    let outcome = flush_queue(&db, &client(&server), &|| false).unwrap();
    assert_eq!(
        outcome,
        QueueFlush {
            sent: 2,
            ..QueueFlush::default()
        }
    );
    assert_eq!(
        submissions(&server),
        [
            ("tr-1".into(), true, Some(1_000)),
            ("tr-2".into(), true, Some(2_000))
        ]
    );
    assert!(queued(&db).is_empty());
}

#[test]
fn flush_sends_at_most_one_batch() {
    let server = server();
    let db = Db::open_in_memory().unwrap();
    for n in 0..(FLUSH_BATCH as i64 + 5) {
        enqueue(&db, "navidrome:tr-1", n);
    }
    let outcome = flush_queue(&db, &client(&server), &|| false).unwrap();
    assert_eq!(outcome.sent, FLUSH_BATCH);
    assert_eq!(queued(&db).len(), 5);
}

#[test]
fn offline_and_account_errors_keep_the_queue() {
    for fault in [
        Fault::Status(503),
        Fault::Refuse,
        Fault::WrongPassword,
        Fault::Code(41),
        Fault::Html,
    ] {
        let server = server();
        let db = Db::open_in_memory().unwrap();
        enqueue(&db, "navidrome:tr-1", 1_000);
        enqueue(&db, "navidrome:tr-2", 2_000);
        server.set_fault(fault.clone());
        let outcome = flush_queue(&db, &client(&server), &|| false).unwrap();
        assert!(outcome.stopped.is_some(), "{fault:?}");
        assert_eq!(outcome.sent + outcome.dropped, 0, "{fault:?}");
        assert_eq!(queued(&db).len(), 2, "{fault:?}");
    }
}

#[test]
fn a_refused_scrobble_is_dropped_and_the_rest_goes_on() {
    let server = server();
    let db = Db::open_in_memory().unwrap();
    enqueue(&db, "navidrome:gone", 1_000);
    enqueue(&db, "navidrome:tr-1", 2_000);
    server.set_fault(Fault::CodeOn {
        method: "scrobble",
        code: 70,
    });
    let outcome = flush_queue(&db, &client(&server), &|| false).unwrap();
    assert_eq!(outcome.dropped, 2);
    assert_eq!(outcome.stopped, None);
    assert!(queued(&db).is_empty());
    // A row without a Navidrome id never reaches the server.
    server.set_fault(Fault::None);
    enqueue(&db, "local-1", 3_000);
    let outcome = flush_queue(&db, &client(&server), &|| false).unwrap();
    assert_eq!(outcome.dropped, 1);
    assert!(submissions(&server).is_empty());
}

#[test]
fn a_cancelled_flush_writes_nothing() {
    let server = server();
    let db = Db::open_in_memory().unwrap();
    enqueue(&db, "navidrome:tr-1", 1_000);
    let outcome = flush_queue(&db, &client(&server), &|| true).unwrap();
    assert_eq!(outcome, QueueFlush::default());
    assert_eq!(queued(&db).len(), 1);
}

// The scrobbler on a running service.

fn options() -> ServiceOptions {
    ServiceOptions {
        first_delay: Duration::from_millis(20),
        interval: Duration::from_secs(3600),
        offline_retry: Duration::from_millis(100),
        ..ServiceOptions::default()
    }
}

struct Env {
    _dir: TempDir,
    db: Arc<Db>,
    service: Arc<NavidromeService>,
}

fn save_account(dir: &Path, server: &FakeServer, creds: &MemoryStore) {
    SettingsStore::load(&dir.join("settings.json"))
        .set_navidrome_account(Some((server.url(), FAKE_USER.into())))
        .unwrap();
    creds
        .set(&server.url(), FAKE_USER, &Secret::new(FAKE_PASSWORD))
        .unwrap();
}

/// A service over the database at `db_path` (in memory when `None`), signed in to `server`
/// when given.
fn start(server: Option<&FakeServer>, db_path: Option<&Path>) -> Env {
    let dir = TempDir::new().unwrap();
    let creds = Arc::new(MemoryStore::new());
    if let Some(server) = server {
        save_account(dir.path(), server, &creds);
    }
    let db = Arc::new(match db_path {
        Some(path) => Db::open(path).unwrap(),
        None => Db::open_in_memory().unwrap(),
    });
    let settings = Arc::new(SettingsStore::load(&dir.path().join("settings.json")));
    let service = NavidromeService::spawn_with_options(
        Arc::clone(&db),
        settings,
        creds as Arc<dyn CredentialStore>,
        Arc::new(|_| {}),
        options(),
    );
    Env {
        _dir: dir,
        db,
        service,
    }
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

impl Env {
    fn wait_online(&self) {
        wait_for("Online", || {
            self.service.status().state == LinkState::Online
        });
        assert!(self.service.wait_idle(WAIT));
    }
}

/// A scrobbler sink on `service` whose clock the test moves.
struct Sink {
    sink: Option<EventSink>,
    thread: Option<JoinHandle<()>>,
    clock: Arc<AtomicI64>,
}

impl Sink {
    fn new(service: &Arc<NavidromeService>) -> Sink {
        let clock = Arc::new(AtomicI64::new(START));
        let read = Arc::clone(&clock);
        let (sink, thread) = spawn_with_clock(
            Arc::clone(service),
            Arc::new(move || read.load(Ordering::SeqCst)),
        );
        Sink {
            sink: Some(sink),
            thread,
            clock,
        }
    }

    fn send(&self, event: PlayerEvent) {
        (self.sink.as_ref().unwrap())(event);
    }

    /// The track becomes audible and plays `ms` in real (test) time.
    fn listen(&self, song: &Track, ms: u64) {
        self.send(PlayerEvent::TrackChanged(song.clone()));
        self.play(song, 0, ms);
    }

    /// Playing states from `from` to `to`, 250 ms apart.
    fn play(&self, song: &Track, from: u64, to: u64) {
        let mut position = from;
        while position < to {
            position += 250;
            self.clock.fetch_add(250, Ordering::SeqCst);
            self.send(PlayerEvent::State(state(
                song,
                PlayerStatus::Playing,
                position,
            )));
        }
    }

    /// Drops the sink and waits for the thread to work through every event.
    fn finish(mut self) {
        self.sink = None;
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn wait_cycle(env: &Env, server: &FakeServer, before: usize) {
    wait_for("a cycle", || {
        server
            .requests()
            .iter()
            .filter(|r| r.method == "getIndexes")
            .count()
            > before
    });
    assert!(env.service.wait_idle(WAIT));
}

fn index_calls(server: &FakeServer) -> usize {
    server
        .requests()
        .iter()
        .filter(|r| r.method == "getIndexes")
        .count()
}

#[test]
fn now_playing_and_submission_reach_the_server() {
    let server = server();
    let env = start(Some(&server), None);
    env.wait_online();
    let sink = Sink::new(&env.service);
    sink.listen(&track("navidrome:tr-1", 60_000), 31_000);
    sink.finish();
    assert_eq!(
        server.scrobbles(),
        [
            ("tr-1".into(), false, Some(START)),
            ("tr-1".into(), true, Some(START))
        ]
    );
    assert!(queued(&env.db).is_empty());
}

#[test]
fn offline_submission_is_queued_and_flushed() {
    let server = server();
    let env = start(Some(&server), None);
    env.wait_online();
    server.set_fault(Fault::Status(503));
    let sink = Sink::new(&env.service);
    sink.listen(&track("navidrome:tr-1", 60_000), 31_000);
    sink.finish();
    assert_eq!(queued(&env.db), [("navidrome:tr-1".into(), START)]);
    assert!(server.scrobbles().is_empty(), "now playing is not retried");

    server.set_fault(Fault::None);
    let before = index_calls(&server);
    env.service.sync_now();
    wait_cycle(&env, &server, before);
    assert_eq!(server.scrobbles(), [("tr-1".into(), true, Some(START))]);
    assert!(queued(&env.db).is_empty());
}

#[test]
fn queued_scrobbles_survive_a_restart() {
    let server = server();
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("library.db");
    {
        let env = start(Some(&server), Some(&db_path));
        env.wait_online();
        server.set_fault(Fault::Status(503));
        let sink = Sink::new(&env.service);
        sink.listen(&track("navidrome:tr-1", 60_000), 31_000);
        sink.finish();
        assert_eq!(queued(&env.db).len(), 1);
        env.service.shutdown();
    }
    server.set_fault(Fault::None);
    let env = start(Some(&server), Some(&db_path));
    env.wait_online();
    wait_for("the queue sent", || queued(&env.db).is_empty());
    assert_eq!(server.scrobbles(), [("tr-1".into(), true, Some(START))]);
}

#[test]
fn local_tracks_never_reach_the_server() {
    let server = server();
    let env = start(Some(&server), None);
    env.wait_online();
    let sink = Sink::new(&env.service);
    sink.listen(&track("local-1", 60_000), 60_000);
    sink.finish();
    assert!(server.scrobbles().is_empty());
    assert!(queued(&env.db).is_empty());
}

#[test]
fn nothing_is_sent_or_queued_after_shutdown() {
    let server = server();
    let env = start(Some(&server), None);
    env.wait_online();
    env.service.shutdown();
    let sink = Sink::new(&env.service);
    sink.listen(&track("navidrome:tr-1", 60_000), 31_000);
    sink.finish();
    assert!(server.scrobbles().is_empty());
    assert!(queued(&env.db).is_empty());
}

#[test]
fn nothing_is_sent_or_queued_while_signed_out() {
    let env = start(None, None);
    let sink = Sink::new(&env.service);
    sink.listen(&track("navidrome:tr-1", 60_000), 31_000);
    sink.finish();
    assert!(queued(&env.db).is_empty());
}

#[test]
fn a_play_of_the_old_server_never_reaches_the_new_one() {
    let first = server();
    let second = server();
    let env = start(Some(&first), None);
    env.wait_online();
    let song = track("navidrome:tr-1", 60_000);
    let sink = Sink::new(&env.service);
    // Announced on the first server; the switch comes before the play counts.
    sink.listen(&song, 10_000);
    wait_for("now playing", || !first.scrobbles().is_empty());
    env.service
        .connect(&second.url(), FAKE_USER, Secret::new(FAKE_PASSWORD))
        .unwrap();
    env.wait_online();
    sink.play(&song, 10_000, 40_000);
    sink.finish();
    assert_eq!(first.scrobbles(), [("tr-1".into(), false, Some(START))]);
    assert!(submissions(&second).is_empty());
    assert!(queued(&env.db).is_empty());
}

#[test]
fn the_switch_wipes_the_queue_and_an_old_epoch_writes_nothing() {
    let first = server();
    let second = server();
    let env = start(Some(&first), None);
    env.wait_online();
    let old_epoch = env.service.core.inner().epoch;
    assert!(env
        .service
        .core
        .queue_scrobble(old_epoch, "navidrome:tr-1", 1_000)
        .unwrap());
    // The first server is down, so the queue waits for it.
    first.set_fault(Fault::Status(503));
    env.service
        .connect(&second.url(), FAKE_USER, Secret::new(FAKE_PASSWORD))
        .unwrap();
    assert!(queued(&env.db).is_empty(), "the switch wipes the queue");
    // A submission of the old account that failed while the switch ran is not queued.
    assert!(!env
        .service
        .core
        .queue_scrobble(old_epoch, "navidrome:tr-2", 2_000)
        .unwrap());
    env.wait_online();
    assert!(queued(&env.db).is_empty());
    assert!(second.scrobbles().is_empty());
    assert!(first.scrobbles().is_empty());
}

#[test]
fn an_account_error_on_the_flush_ends_the_cycle_and_keeps_the_queue() {
    let server = server();
    let env = start(Some(&server), None);
    env.wait_online();
    enqueue(&env.db, "navidrome:tr-1", 1_000);
    server.set_fault(Fault::CodeOn {
        method: "scrobble",
        code: 40,
    });
    let before = index_calls(&server);
    env.service.sync_now();
    wait_cycle(&env, &server, before);
    let status = env.service.status();
    assert_eq!(status.state, LinkState::Error);
    assert_eq!(status.error.unwrap().code, "wrongCredentials");
    assert_eq!(queued(&env.db).len(), 1);
}

#[test]
fn an_offline_flush_ends_the_cycle_step_and_keeps_the_queue() {
    let server = server();
    let env = start(Some(&server), None);
    env.wait_online();
    enqueue(&env.db, "navidrome:tr-1", 1_000);
    let client = env.service.client().unwrap();
    let epoch = env.service.core.inner().epoch;
    server.set_fault(Fault::Status(503));
    match env.service.core.flush_scrobbles(&client, epoch) {
        Err(SyncError::Api(err)) => assert!(err.is_offline(), "{err:?}"),
        other => panic!("expected offline, got {other:?}"),
    }
    assert_eq!(queued(&env.db).len(), 1);
    // A step of an old epoch is cancelled before it writes.
    server.set_fault(Fault::None);
    assert!(matches!(
        env.service.core.flush_scrobbles(&client, epoch + 1),
        Err(SyncError::Cancelled)
    ));
    assert_eq!(queued(&env.db).len(), 1);
    assert!(server.scrobbles().is_empty());
}

#[test]
fn a_refused_scrobble_does_not_halt_the_cycle() {
    let server = server();
    let env = start(Some(&server), None);
    env.wait_online();
    enqueue(&env.db, "navidrome:tr-1", 1_000);
    server.set_fault(Fault::CodeOn {
        method: "scrobble",
        code: 0,
    });
    let before = index_calls(&server);
    env.service.sync_now();
    wait_cycle(&env, &server, before);
    let status = env.service.status();
    assert_eq!(status.state, LinkState::Online);
    assert_eq!(status.error, None);
    assert!(queued(&env.db).is_empty(), "dropped, not retried forever");
}

#[test]
fn ensure_server_wipe_takes_the_queue() {
    let db = Db::open_in_memory().unwrap();
    assert!(ensure_server(&db, &server_key("http://127.0.0.1:1", FAKE_USER)).unwrap());
    enqueue(&db, "navidrome:tr-1", 1_000);
    assert!(ensure_server(&db, &server_key("http://127.0.0.1:2", FAKE_USER)).unwrap());
    assert!(queued(&db).is_empty());
}
