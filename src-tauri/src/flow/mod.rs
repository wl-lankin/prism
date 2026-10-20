//! Flow: an endless queue dealt from a pool of tracks (favorites, the library or one genre).

pub mod deck;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crossbeam_channel::Sender;
use rand::rngs::StdRng;
use rand::SeedableRng;
use rusqlite::{params_from_iter, Connection};
use serde::{Deserialize, Serialize};

pub use deck::Deck;

use crate::db::Db;
use crate::library::scan::lock;
use crate::player::{EventSink, PlayerEvent, PlayerHandle, PlayerState};

/// How many tracks a Flow deals at once: the first queue, and every refill.
pub const FLOW_BATCH: usize = 50;
/// A Flow refills once `queue_length - queue_index` is down to this.
pub const FLOW_REFILL_AT: u32 = 10;

/// What `flow_start` did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "lowercase")]
pub enum FlowOutcome {
    /// Playing; `variant` is what actually plays (favorites without any fall back to the
    /// library).
    Started { variant: FlowVariant },
    /// The library has no tracks; the player was not touched.
    Empty,
}

/// The running Flow, owned by the service thread.
struct Session {
    variant: FlowVariant,
    deck: Deck,
    /// The queue version before the Flow's play: older states are ignored.
    after_version: u64,
    /// The queue version an append in flight was sent at; no second refill until a state
    /// with a newer version arrives.
    awaiting: Option<u64>,
    /// A state of this Flow (`flow == Some(variant)`) has been seen. Until then a state of
    /// another queue is a queue change that raced the Flow's play and is ignored.
    seen: bool,
}

enum Message {
    Start(Box<Session>),
    State(Box<PlayerState>),
}

/// Keeps a Flow going: deals the first batch and, on its own thread, appends another batch
/// whenever the queue nears its end, for as long as the player says the queue is still that
/// Flow's (`PlayerState::flow`). Any other play ends the session.
pub struct FlowService {
    db: Arc<Db>,
    player: PlayerHandle,
    messages: Sender<Message>,
    /// One start at a time, so sessions reach the service thread in the order they played.
    starting: Mutex<()>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

impl FlowService {
    /// Starts the service thread. The returned sink must receive the player's events (it
    /// only forwards states to the service thread, so the engine never waits on a refill).
    pub fn spawn(db: Arc<Db>, player: PlayerHandle) -> (Arc<FlowService>, EventSink) {
        let (messages, received) = crossbeam_channel::unbounded::<Message>();
        let refill_player = player.clone();
        let spawned = std::thread::Builder::new()
            .name("prism-flow".into())
            .spawn(move || {
                let mut session: Option<Session> = None;
                for message in received {
                    match message {
                        Message::Start(started) => session = Some(*started),
                        Message::State(state) => {
                            if let Some(current) = session.as_mut() {
                                if !refill(current, &state, |ids| refill_player.append(ids)) {
                                    session = None;
                                }
                            }
                        }
                    }
                }
            });
        if let Err(err) = spawned {
            // Flows still start; they only end after their first batch.
            eprintln!("starting the Flow service failed: {err}");
        }
        let states = messages.clone();
        let sink: EventSink = Arc::new(move |event| {
            if let PlayerEvent::State(state) = event {
                let _ = states.send(Message::State(Box::new(state)));
            }
        });
        let service = Arc::new(FlowService {
            db,
            player,
            messages,
            starting: Mutex::new(()),
        });
        (service, sink)
    }

    /// Plays a new Flow of `variant` (favorites without any fall back to the library).
    /// Reads the library, so call it off the UI thread.
    pub fn start(&self, variant: FlowVariant) -> anyhow::Result<FlowOutcome> {
        let _one_at_a_time = self.starting.lock().unwrap_or_else(|p| p.into_inner());
        let (variant, candidates) = {
            let conn = lock(&self.db)?;
            let candidates = pool(&conn, &variant)?;
            if candidates.is_empty() && variant == FlowVariant::Favorites {
                (FlowVariant::Library, pool(&conn, &FlowVariant::Library)?)
            } else {
                (variant, candidates)
            }
        };
        if candidates.is_empty() {
            return Ok(FlowOutcome::Empty);
        }
        let mut deck = Deck::new(candidates, now_ms(), StdRng::from_os_rng());
        let first = deck.next_batch(FLOW_BATCH);
        let after_version = self.player.state().queue_version;
        self.player
            .play_with(first, 0, Some(variant.clone()))
            .map_err(anyhow::Error::msg)?;
        let session = Session {
            variant: variant.clone(),
            deck,
            after_version,
            awaiting: None,
            seen: false,
        };
        // Only fails once the service thread is gone; the Flow then plays its first batch.
        let _ = self.messages.send(Message::Start(Box::new(session)));
        Ok(FlowOutcome::Started { variant })
    }
}

/// Acts on one player state for `session`, appending through `append`; false once the queue
/// is no longer this Flow's.
fn refill(
    session: &mut Session,
    state: &PlayerState,
    append: impl FnOnce(Vec<String>) -> Result<(), String>,
) -> bool {
    if state.queue_version <= session.after_version {
        // From before the Flow's play.
        return true;
    }
    if state.flow.as_ref() != Some(&session.variant) {
        // Before the Flow's own first state, another queue is a change that raced its play
        // (the version was read before it): only a later one ends the Flow.
        return !session.seen;
    }
    session.seen = true;
    if let Some(version) = session.awaiting {
        // Every append bumps the queue version: a newer state already holds it.
        if state.queue_version <= version {
            return true;
        }
        session.awaiting = None;
    }
    if state.queue_length.saturating_sub(state.queue_index) > FLOW_REFILL_AT {
        return true;
    }
    let batch = session.deck.next_batch(FLOW_BATCH);
    if !batch.is_empty() && append(batch).is_ok() {
        session.awaiting = Some(state.queue_version);
    }
    true
}

/// Where a Flow draws its tracks from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum FlowVariant {
    Favorites,
    Library,
    Genre { genre: String },
}

/// One track a Flow may deal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub artist: String,
    /// Unix milliseconds of the last play, `None` when never played.
    pub last_played: Option<i64>,
}

/// A pool smaller than this is padded from the library.
pub const PAD_TARGET: usize = 100;

const CANDIDATE_SELECT: &str = "SELECT t.id, t.artist, s.last_played, t.genre
     FROM tracks t LEFT JOIN track_stats s ON s.track_id = t.id";

/// A candidate with the track's genre (padding needs the pool's genres).
type Row = (Candidate, Option<String>);

fn rows<P: rusqlite::Params>(
    conn: &Connection,
    filter: &str,
    params: P,
    mut take: impl FnMut(Row) -> bool,
) -> anyhow::Result<()> {
    let mut stmt = conn.prepare(&format!("{CANDIDATE_SELECT} {filter} ORDER BY t.row_id"))?;
    let mut found = stmt.query(params)?;
    while let Some(row) = found.next()? {
        let candidate = Candidate {
            id: row.get(0)?,
            artist: row.get(1)?,
            last_played: row.get(2)?,
        };
        if !take((candidate, row.get(3)?)) {
            break;
        }
    }
    Ok(())
}

fn placeholders(count: usize) -> String {
    vec!["?"; count].join(", ")
}

/// The tracks a Flow of `variant` deals from, each once. Favorites: favorite tracks, every
/// track of a favorite album, and every track whose artist or album artist is a favorite
/// artist (exact name). A favorites pool under `PAD_TARGET` is padded from the library:
/// first tracks by the pool's artists, then tracks of the pool's genres. A genre pool is
/// never padded.
pub fn pool(conn: &Connection, variant: &FlowVariant) -> anyhow::Result<Vec<Candidate>> {
    let mut found: Vec<Row> = Vec::new();
    let push = |row: Row| {
        found.push(row);
        true
    };
    match variant {
        FlowVariant::Library => rows(conn, "", [], push)?,
        FlowVariant::Favorites => rows(
            conn,
            "WHERE t.id IN (SELECT item_id FROM favorites WHERE kind = 'track')
                OR t.album_id IN (SELECT item_id FROM favorites WHERE kind = 'album')
                OR t.artist IN (SELECT item_id FROM favorites WHERE kind = 'artist')
                OR t.album_artist IN (SELECT item_id FROM favorites WHERE kind = 'artist')",
            [],
            push,
        )?,
        FlowVariant::Genre { genre } => rows(
            conn,
            "WHERE t.genre = ?1 COLLATE NOCASE",
            [genre.as_str()],
            push,
        )?,
    }
    // Only favorites are padded: a genre Flow stays in its genre however small, and the
    // library already holds everything.
    if *variant == FlowVariant::Favorites && found.len() < PAD_TARGET {
        pad(conn, &mut found)?;
    }
    Ok(found.into_iter().map(|(candidate, _)| candidate).collect())
}

fn pad(conn: &Connection, found: &mut Vec<Row>) -> anyhow::Result<()> {
    let mut seen: HashSet<String> = found.iter().map(|(c, _)| c.id.clone()).collect();
    let mut artists: Vec<String> = Vec::new();
    let mut genres: Vec<String> = Vec::new();
    let mut genre_keys: HashSet<String> = HashSet::new();
    for (candidate, genre) in found.iter() {
        if !artists.contains(&candidate.artist) {
            artists.push(candidate.artist.clone());
        }
        if let Some(genre) = genre.as_deref().map(str::trim).filter(|g| !g.is_empty()) {
            if genre_keys.insert(genre.to_lowercase()) {
                genres.push(genre.to_string());
            }
        }
    }
    let passes = [
        (
            format!("WHERE t.artist IN ({})", placeholders(artists.len())),
            artists,
        ),
        (
            format!(
                "WHERE t.genre COLLATE NOCASE IN ({})",
                placeholders(genres.len())
            ),
            genres,
        ),
    ];
    for (filter, values) in &passes {
        if found.len() >= PAD_TARGET || values.is_empty() {
            continue;
        }
        rows(conn, filter, params_from_iter(values), |row| {
            if seen.insert(row.0.id.clone()) {
                found.push(row);
            }
            found.len() < PAD_TARGET
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rusqlite::params;

    use super::*;
    use crate::db::Db;

    fn add(
        conn: &Connection,
        id: &str,
        artist: &str,
        album_artist: &str,
        album_id: &str,
        genre: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id,
                duration_ms, format, genre)
             VALUES (?1, 'local', ?1, ?2, ?3, ?4, ?4, 1000, 'flac', ?5)",
            params![id, artist, album_artist, album_id, genre],
        )
        .unwrap();
    }

    fn favorite(conn: &Connection, kind: &str, id: &str) {
        conn.execute(
            "INSERT INTO favorites (kind, item_id, added_at) VALUES (?1, ?2, 1)",
            params![kind, id],
        )
        .unwrap();
    }

    fn ids(pool: &[Candidate]) -> Vec<&str> {
        let mut ids: Vec<&str> = pool.iter().map(|c| c.id.as_str()).collect();
        ids.sort();
        ids
    }

    #[test]
    fn favorite_album_contributes_all_tracks() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        for i in 0..3 {
            add(&conn, &format!("fav-{i}"), "A", "A", "album-fav", None);
        }
        add(&conn, "other", "B", "B", "album-other", None);
        favorite(&conn, "album", "album-fav");
        // A track that is also a favorite on its own is listed once.
        favorite(&conn, "track", "fav-1");
        let pool = pool(&conn, &FlowVariant::Favorites).unwrap();
        assert_eq!(ids(&pool), vec!["fav-0", "fav-1", "fav-2"]);
    }

    #[test]
    fn favorite_artist_contributes_all_tracks() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        add(
            &conn,
            "by-artist",
            "Portishead",
            "Portishead",
            "dummy",
            None,
        );
        add(
            &conn,
            "on-compilation",
            "Various",
            "Portishead",
            "live",
            None,
        );
        add(&conn, "featured", "Portishead", "Various", "mix", None);
        add(
            &conn,
            "case-differs",
            "portishead",
            "portishead",
            "other",
            None,
        );
        add(&conn, "stranger", "Tricky", "Tricky", "maxinquaye", None);
        favorite(&conn, "artist", "Portishead");
        let pool = pool(&conn, &FlowVariant::Favorites).unwrap();
        assert_eq!(ids(&pool), vec!["by-artist", "featured", "on-compilation"]);
    }

    #[test]
    fn small_pool_is_padded() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        for i in 0..150 {
            let artist = format!("Artist {}", i % 5);
            add(
                &conn,
                &format!("t{i:03}"),
                &artist,
                &artist,
                &format!("album {}", i % 5),
                Some("Rock"),
            );
        }
        favorite(&conn, "track", "t007");
        conn.execute(
            "INSERT INTO track_stats (track_id, play_count, last_played) VALUES ('t012', 1, 99)",
            [],
        )
        .unwrap();
        let pool = pool(&conn, &FlowVariant::Favorites).unwrap();
        assert_eq!(pool.len(), PAD_TARGET);
        assert_eq!(pool[0].id, "t007");
        assert!(
            pool[1..30].iter().all(|c| c.artist == "Artist 2"),
            "{pool:?}"
        );
        assert_eq!(pool.iter().filter(|c| c.artist == "Artist 2").count(), 30);
        assert_eq!(ids(&pool).windows(2).filter(|w| w[0] == w[1]).count(), 0);
        let played = pool
            .iter()
            .find(|c| c.id == "t012")
            .expect("padded with t012");
        assert_eq!(played.last_played, Some(99));
    }

    #[test]
    fn genre_pool_only_that_genre() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        add(&conn, "r1", "R", "R", "r", Some("Rock"));
        add(&conn, "r2", "R", "R", "r", Some("rock"));
        add(&conn, "r3", "S", "S", "s", Some("ROCK"));
        add(&conn, "j1", "J", "J", "j", Some("Jazz"));
        add(&conn, "n1", "N", "N", "n", None);
        // Same artist and album as the rock tracks, another genre: not dealt.
        add(&conn, "r4", "R", "R", "r", Some("Jazz"));
        let pool = pool(
            &conn,
            &FlowVariant::Genre {
                genre: "Rock".into(),
            },
        )
        .unwrap();
        assert_eq!(ids(&pool), vec!["r1", "r2", "r3"]);
    }

    #[test]
    fn small_genre_pool_is_not_padded() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        for i in 0..150 {
            let artist = format!("Artist {}", i % 5);
            let genre = if i < 12 { "Folk" } else { "Jazz" };
            add(
                &conn,
                &format!("t{i:03}"),
                &artist,
                &artist,
                &format!("album {}", i % 5),
                Some(genre),
            );
        }
        let pool = pool(
            &conn,
            &FlowVariant::Genre {
                genre: "folk".into(),
            },
        )
        .unwrap();
        let expected: Vec<String> = (0..12).map(|i| format!("t{i:03}")).collect();
        assert_eq!(
            ids(&pool),
            expected.iter().map(String::as_str).collect::<Vec<_>>()
        );
    }

    mod refill_rules {
        use rand::SeedableRng;

        use super::super::*;

        fn session() -> Session {
            let pool = (0..120)
                .map(|i| Candidate {
                    id: format!("t{i:03}"),
                    artist: format!("A{}", i % 7),
                    last_played: None,
                })
                .collect();
            Session {
                variant: FlowVariant::Library,
                deck: Deck::new(pool, 0, StdRng::seed_from_u64(3)),
                after_version: 4,
                awaiting: None,
                seen: false,
            }
        }

        fn state(version: u64, index: u32, length: u32) -> PlayerState {
            PlayerState {
                status: crate::player::PlayerStatus::Playing,
                track: None,
                position_ms: 0,
                duration_ms: 0,
                volume: 1.0,
                queue_index: index,
                queue_length: length,
                queue_version: version,
                flow: Some(FlowVariant::Library),
            }
        }

        /// Runs `refill` and returns (still the session, how many ids were appended).
        fn step(session: &mut Session, state: &PlayerState) -> (bool, usize) {
            let mut appended = 0;
            let alive = refill(session, state, |ids| {
                appended = ids.len();
                Ok(())
            });
            (alive, appended)
        }

        #[test]
        fn states_from_before_the_play_are_ignored() {
            let mut s = session();
            // The previous queue was near its end, but that was before this Flow's play.
            assert_eq!(step(&mut s, &state(4, 95, 100)), (true, 0));
            assert_eq!(step(&mut s, &state(3, 99, 100)), (true, 0));
            assert!(s.awaiting.is_none());
        }

        #[test]
        fn refills_once_per_append_then_again_after_it_lands() {
            let mut s = session();
            assert_eq!(step(&mut s, &state(5, 0, 50)), (true, 0));
            assert_eq!(step(&mut s, &state(5, 40, 50)), (true, FLOW_BATCH));
            // The append is in flight: the next position updates do not send another.
            assert_eq!(step(&mut s, &state(5, 41, 50)), (true, 0));
            // Landed (newer version): far from the end again.
            assert_eq!(step(&mut s, &state(6, 42, 100)), (true, 0));
            assert!(s.awaiting.is_none());
            assert_eq!(step(&mut s, &state(6, 90, 100)), (true, FLOW_BATCH));
        }

        #[test]
        fn a_stale_state_does_not_stall_the_refill() {
            // A restart of the same Flow seen through a stale state of the old, longer
            // queue: the append it sends is judged by version, not by the old length.
            let mut s = session();
            assert_eq!(step(&mut s, &state(5, 95, 100)), (true, FLOW_BATCH));
            // The fresh queue (50 long, version bumped by the append) is seen next.
            assert_eq!(step(&mut s, &state(6, 0, 100)), (true, 0));
            assert!(s.awaiting.is_none(), "the append landed");
            assert_eq!(step(&mut s, &state(6, 91, 100)), (true, FLOW_BATCH));
        }

        #[test]
        fn another_queue_ends_the_session() {
            let mut s = session();
            assert_eq!(step(&mut s, &state(5, 0, 50)), (true, 0));
            let mut other = state(6, 0, 2);
            other.flow = None;
            assert_eq!(step(&mut s, &other), (false, 0));
        }

        #[test]
        fn a_queue_change_racing_the_start_does_not_end_a_fresh_flow() {
            let mut s = session();
            // Another play landed after the version was read but before the Flow's play.
            let mut racing = state(5, 0, 2);
            racing.flow = None;
            assert_eq!(step(&mut s, &racing), (true, 0));
            let mut other_flow = state(5, 1, 2);
            other_flow.flow = Some(FlowVariant::Favorites);
            assert_eq!(step(&mut s, &other_flow), (true, 0));
            assert!(!s.seen);
            // The Flow's own queue arrives and runs as usual.
            assert_eq!(step(&mut s, &state(6, 0, 50)), (true, 0));
            assert!(s.seen);
            assert_eq!(step(&mut s, &state(6, 40, 50)), (true, FLOW_BATCH));
            // From now on another queue ends it.
            let mut later = state(8, 0, 2);
            later.flow = None;
            assert_eq!(step(&mut s, &later), (false, 0));
        }
    }

    #[test]
    fn library_pool_holds_every_track() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        add(&conn, "a", "A", "A", "x", None);
        add(&conn, "b", "B", "B", "y", Some("Jazz"));
        let pool = pool(&conn, &FlowVariant::Library).unwrap();
        assert_eq!(ids(&pool), vec!["a", "b"]);
    }

    /// The service against a real engine with a fake output that plays `SPEED` times faster
    /// than real time; every track is `TONE_MS` of silence.
    mod service {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        use crossbeam_channel::{Receiver, Sender};

        use super::*;
        use crate::audio::{AudioSource, OutputEvent, OutputFactory, OutputSink, StreamSpec};
        use crate::library::model::Track;
        use crate::library::query;
        use crate::library::scan::lock;
        use crate::player::{
            fan_out, EventSink, Opener, Player, PlayerEvent, PlayerOptions, PlayerState,
            PlayerStatus, Resolver,
        };

        const SPEC: StreamSpec = StreamSpec {
            sample_rate: 44_100,
            channels: 2,
        };
        const TONE_MS: u64 = 200;
        const SPEED: f64 = 40.0;
        /// The most the fake device takes ahead (like a real ring, it banks no idle time).
        const RING_MS: f64 = 100.0;

        struct FastSink {
            last: Instant,
            room: f64,
            events: Receiver<OutputEvent>,
            _keep: Sender<OutputEvent>,
        }

        impl OutputSink for FastSink {
            fn spec(&self) -> StreamSpec {
                SPEC
            }
            fn push(&mut self, samples: &[f32]) -> usize {
                let rate = SPEC.sample_rate as f64;
                let now = Instant::now();
                self.room = (self.room
                    + now.duration_since(self.last).as_secs_f64() * SPEED * rate)
                    .min(rate * RING_MS / 1000.0);
                self.last = now;
                let frames = (self.room as usize).min(samples.len() / 2);
                self.room -= frames as f64;
                frames * 2
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

        struct FastFactory;

        impl OutputFactory for FastFactory {
            fn open(&self, _device: Option<String>) -> anyhow::Result<Box<dyn OutputSink>> {
                let (tx, rx) = crossbeam_channel::unbounded();
                Ok(Box::new(FastSink {
                    last: Instant::now(),
                    room: 0.0,
                    events: rx,
                    _keep: tx,
                }))
            }
        }

        struct Silence(u64);

        impl AudioSource for Silence {
            fn spec(&self) -> StreamSpec {
                SPEC
            }
            fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize> {
                let frames = self.0.min(1024);
                out.extend(std::iter::repeat_n(0.0, frames as usize * 2));
                self.0 -= frames;
                Ok(frames as usize)
            }
            fn seek(&mut self, ms: u64) -> anyhow::Result<()> {
                self.0 = TONE_MS.saturating_sub(ms) * 44_100 / 1000;
                Ok(())
            }
            fn duration_ms(&self) -> Option<u64> {
                Some(TONE_MS)
            }
        }

        struct Rig {
            flow: Arc<FlowService>,
            player: Player,
            events: mpsc::Receiver<PlayerEvent>,
        }

        /// A library of `count` tracks by 12 artists, each with a file to "play".
        fn library(count: usize) -> Arc<Db> {
            let db = Db::open_in_memory().unwrap();
            {
                let conn = db.0.lock().unwrap();
                for i in 0..count {
                    let artist = format!("Artist {}", i % 12);
                    conn.execute(
                        "INSERT INTO tracks (id, source, title, artist, album_artist, album,
                            album_id, duration_ms, format, path)
                         VALUES (?1, 'local', ?1, ?2, ?2, 'Album', ?2, ?3, 'flac', ?1)",
                        params![format!("t{i:03}"), artist, TONE_MS as i64],
                    )
                    .unwrap();
                }
            }
            Arc::new(db)
        }

        fn rig(db: Arc<Db>) -> Rig {
            let resolver_db = Arc::clone(&db);
            let resolver: Resolver = Arc::new(move |id: &str| -> Option<Track> {
                let conn = lock(&resolver_db).ok()?;
                query::track(&conn, id).ok().flatten()
            });
            let opener: Opener = Arc::new(|_track: &Track, _spec: StreamSpec| {
                Ok(Box::new(Silence(TONE_MS * 44_100 / 1000)) as Box<dyn AudioSource>)
            });
            let (tx, events) = mpsc::channel();
            let tx = std::sync::Mutex::new(tx);
            let record: EventSink = Arc::new(move |event| {
                let _ = tx.lock().unwrap().send(event);
            });
            let late: Arc<std::sync::OnceLock<EventSink>> = Arc::default();
            let forward = Arc::clone(&late);
            let player = Player::spawn_with_options(
                Arc::new(FastFactory),
                resolver,
                fan_out(vec![
                    record,
                    Arc::new(move |event| {
                        if let Some(sink) = forward.get() {
                            sink(event);
                        }
                    }),
                ]),
                opener,
                PlayerOptions::default(),
            );
            let (flow, sink) = FlowService::spawn(db, player.handle());
            assert!(late.set(sink).is_ok());
            Rig {
                flow,
                player,
                events,
            }
        }

        #[track_caller]
        fn wait_state(player: &Player, ok: impl Fn(&PlayerState) -> bool) -> PlayerState {
            let deadline = Instant::now() + Duration::from_secs(30);
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
        fn flow_refills_near_the_end() {
            let rig = rig(library(120));
            let outcome = rig.flow.start(FlowVariant::Library).unwrap();
            assert_eq!(
                outcome,
                FlowOutcome::Started {
                    variant: FlowVariant::Library
                }
            );
            let first = wait_state(&rig.player, |s| s.queue_length > 0);
            assert_eq!(first.queue_length as usize, FLOW_BATCH);
            assert_eq!(first.flow, Some(FlowVariant::Library));
            // Played through the end of the first batch: the queue grew before it ran out.
            let past = wait_state(&rig.player, |s| s.queue_index as usize > FLOW_BATCH + 5);
            assert!(past.queue_length as usize >= 2 * FLOW_BATCH, "{past:#?}");
            assert_eq!(past.flow, Some(FlowVariant::Library));
            let events: Vec<PlayerEvent> = rig.events.try_iter().collect();
            let started = events
                .iter()
                .position(
                    |e| matches!(e, PlayerEvent::State(s) if s.status == PlayerStatus::Playing),
                )
                .expect("it played");
            assert!(
                !events[started..]
                    .iter()
                    .any(|e| matches!(e, PlayerEvent::State(s) if s.status == PlayerStatus::Idle)),
                "no gap: the player never went idle"
            );
            assert!(
                !events.iter().any(|e| matches!(e, PlayerEvent::Error(_))),
                "nothing failed"
            );
            let heard: Vec<String> = events
                .iter()
                .filter_map(|e| match e {
                    PlayerEvent::TrackChanged(track) => Some(track.id.clone()),
                    _ => None,
                })
                .collect();
            let queue = rig.player.queue_ids();
            assert_eq!(
                heard,
                queue[..heard.len()],
                "every track was heard, in order"
            );
            // Near the end again: a third batch follows; the deck never repeats within the pool.
            rig.player.jump(queue.len() - 3).unwrap();
            wait_state(&rig.player, |s| s.queue_length as usize >= 3 * FLOW_BATCH);
            let queue = rig.player.queue_ids();
            let distinct: HashSet<&String> = queue[..120].iter().collect();
            assert_eq!(distinct.len(), 120, "no repeats within the first 120");
        }

        #[test]
        fn playing_an_album_ends_the_flow_session() {
            let rig = rig(library(120));
            rig.flow.start(FlowVariant::Library).unwrap();
            let flowing = wait_state(&rig.player, |s| s.flow.is_some() && s.queue_length > 0);
            rig.player
                .play(vec!["t000".into(), "t001".into()], 0)
                .unwrap();
            let album = wait_state(&rig.player, |s| s.queue_version > flowing.queue_version);
            assert_eq!(album.flow, None, "play clears the Flow");
            // Two tracks: well inside the refill margin, and played to the end.
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                match rig.events.recv_timeout(Duration::from_secs(1)) {
                    Ok(PlayerEvent::State(s)) if s.status == PlayerStatus::Idle => break,
                    Ok(_) => {}
                    Err(_) => {}
                }
                assert!(Instant::now() < deadline, "the album never ended");
            }
            // Give the service a moment to act on the last states, had it still been alive.
            std::thread::sleep(Duration::from_millis(200));
            let state = rig.player.state();
            assert_eq!(state.queue_length, 2, "no more appends: {state:#?}");
            assert_eq!(rig.player.queue_ids(), vec!["t000", "t001"]);
        }

        #[test]
        fn empty_favorites_fall_back_to_library() {
            let rig = rig(library(30));
            let outcome = rig.flow.start(FlowVariant::Favorites).unwrap();
            assert_eq!(
                outcome,
                FlowOutcome::Started {
                    variant: FlowVariant::Library
                }
            );
            let state = wait_state(&rig.player, |s| s.queue_length > 0);
            assert_eq!(state.flow, Some(FlowVariant::Library));
            assert_eq!(state.queue_length as usize, FLOW_BATCH);
            assert_eq!(
                serde_json::to_value(&outcome).unwrap(),
                serde_json::json!({ "outcome": "started", "variant": { "kind": "library" } })
            );
        }

        #[test]
        fn empty_library_is_empty() {
            let rig = rig(library(0));
            for variant in [
                FlowVariant::Favorites,
                FlowVariant::Library,
                FlowVariant::Genre {
                    genre: "Jazz".into(),
                },
            ] {
                assert_eq!(rig.flow.start(variant).unwrap(), FlowOutcome::Empty);
            }
            assert_eq!(
                serde_json::to_value(FlowOutcome::Empty).unwrap(),
                serde_json::json!({ "outcome": "empty" })
            );
            std::thread::sleep(Duration::from_millis(100));
            let state = rig.player.state();
            assert_eq!(state.queue_length, 0, "the player was not touched");
            assert_eq!(state.flow, None);
        }
    }

    #[test]
    fn variant_serde_shape() {
        let genre: FlowVariant =
            serde_json::from_str(r#"{"kind":"genre","genre":"Trip Hop"}"#).unwrap();
        assert_eq!(
            genre,
            FlowVariant::Genre {
                genre: "Trip Hop".into()
            }
        );
        assert_eq!(
            serde_json::to_string(&FlowVariant::Favorites).unwrap(),
            r#"{"kind":"favorites"}"#
        );
    }
}
