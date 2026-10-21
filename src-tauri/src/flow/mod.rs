//! Flow: an endless queue dealt from a pool of tracks (favorites, the library or one genre).

pub mod deck;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// The deck's pool was read with Navidrome tracks (the server answered then).
    with_navidrome: bool,
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
/// Flow's (`PlayerState::flow`). Any other play ends the session. Navidrome tracks are dealt
/// only while the server answers: the flag is read at the start and at every refill.
pub struct FlowService {
    db: Arc<Db>,
    player: PlayerHandle,
    navidrome_online: Arc<AtomicBool>,
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
    /// `navidrome_online` is `NavidromeService::online_flag`.
    pub fn spawn(
        db: Arc<Db>,
        player: PlayerHandle,
        navidrome_online: Arc<AtomicBool>,
    ) -> (Arc<FlowService>, EventSink) {
        let (messages, received) = crossbeam_channel::unbounded::<Message>();
        let refill_player = player.clone();
        let refill_db = Arc::clone(&db);
        let refill_online = Arc::clone(&navidrome_online);
        let spawned = std::thread::Builder::new()
            .name("prism-flow".into())
            .spawn(move || {
                let mut session: Option<Session> = None;
                for message in received {
                    match message {
                        Message::Start(started) => session = Some(*started),
                        Message::State(state) => {
                            if let Some(current) = session.as_mut() {
                                let variant = current.variant.clone();
                                let alive = refill(
                                    current,
                                    &state,
                                    refill_online.load(Ordering::SeqCst),
                                    |include| {
                                        let conn = lock(&refill_db)?;
                                        Ok(deal_pool(&conn, &variant, include)?.1)
                                    },
                                    |ids| refill_player.append(ids),
                                );
                                if !alive {
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
            navidrome_online,
            messages,
            starting: Mutex::new(()),
        });
        (service, sink)
    }

    /// Plays a new Flow of `variant` (favorites without any fall back to the library).
    /// Reads the library, so call it off the UI thread.
    pub fn start(&self, variant: FlowVariant) -> anyhow::Result<FlowOutcome> {
        let _one_at_a_time = self.starting.lock().unwrap_or_else(|p| p.into_inner());
        let with_navidrome = self.navidrome_online.load(Ordering::SeqCst);
        let (variant, candidates) = {
            let conn = lock(&self.db)?;
            deal_pool(&conn, &variant, with_navidrome)?
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
            with_navidrome,
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
/// is no longer this Flow's. `online` is the Navidrome flag now: when it differs from the
/// deck's, the pool is read again through `repool(online)` before dealing.
fn refill(
    session: &mut Session,
    state: &PlayerState,
    online: bool,
    repool: impl FnOnce(bool) -> anyhow::Result<Vec<Candidate>>,
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
    if online != session.with_navidrome {
        match repool(online) {
            Ok(candidates) => {
                session.deck.replace_pool(candidates, now_ms());
                session.with_navidrome = online;
            }
            // Kept: the next refill reads the pool again.
            Err(err) => eprintln!("reading the Flow pool failed: {err:#}"),
        }
    }
    let mut batch = session.deck.next_batch(FLOW_BATCH);
    if !online {
        // Only while a failed read left the online deck in place.
        batch.retain(|id| !is_navidrome(id));
    }
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

/// A Navidrome track id (`navidrome:<song id>`).
fn is_navidrome(id: &str) -> bool {
    id.starts_with("navidrome:")
}

/// A candidate with the track's genre (padding needs the pool's genres).
type Row = (Candidate, Option<String>);

/// The tracks matching `filter` (an SQL condition, empty for every track) in index order,
/// without Navidrome tracks unless `with_navidrome`, handed to `take` until it says stop.
fn rows<P: rusqlite::Params>(
    conn: &Connection,
    filter: &str,
    with_navidrome: bool,
    params: P,
    mut take: impl FnMut(Row) -> bool,
) -> anyhow::Result<()> {
    let mut conditions: Vec<String> = Vec::new();
    if !filter.is_empty() {
        conditions.push(format!("({filter})"));
    }
    if !with_navidrome {
        conditions.push("t.source <> 'navidrome'".into());
    }
    let filter = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };
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

/// The pool a Flow of `variant` deals from and the variant it plays as: favorites without any
/// fall back to the library.
fn deal_pool(
    conn: &Connection,
    variant: &FlowVariant,
    with_navidrome: bool,
) -> anyhow::Result<(FlowVariant, Vec<Candidate>)> {
    let candidates = pool(conn, variant, with_navidrome)?;
    if candidates.is_empty() && *variant == FlowVariant::Favorites {
        let library = pool(conn, &FlowVariant::Library, with_navidrome)?;
        return Ok((FlowVariant::Library, library));
    }
    Ok((variant.clone(), candidates))
}

/// The tracks a Flow of `variant` deals from, each once. Favorites: favorite tracks, every
/// track of a favorite album, every track whose artist or album artist is a favorite artist
/// (exact name, a local artist's id), and every track on an album of a favorite server artist
/// (`albums.artist_id`, a Navidrome artist's id). A favorites pool under `PAD_TARGET` is padded
/// from the library: first tracks by the pool's artists, then tracks of the pool's genres. A
/// genre pool is never padded. Without `with_navidrome` (the server does not answer) neither
/// the pool nor its padding holds a Navidrome track.
pub fn pool(
    conn: &Connection,
    variant: &FlowVariant,
    with_navidrome: bool,
) -> anyhow::Result<Vec<Candidate>> {
    let mut found: Vec<Row> = Vec::new();
    let push = |row: Row| {
        found.push(row);
        true
    };
    match variant {
        FlowVariant::Library => rows(conn, "", with_navidrome, [], push)?,
        FlowVariant::Favorites => rows(
            conn,
            "t.id IN (SELECT item_id FROM favorites WHERE kind = 'track')
                OR t.album_id IN (SELECT item_id FROM favorites WHERE kind = 'album')
                OR t.artist IN (SELECT item_id FROM favorites WHERE kind = 'artist')
                OR t.album_artist IN (SELECT item_id FROM favorites WHERE kind = 'artist')
                OR t.album_id IN (SELECT a.id FROM albums a WHERE a.artist_id IN
                    (SELECT item_id FROM favorites WHERE kind = 'artist'))",
            with_navidrome,
            [],
            push,
        )?,
        FlowVariant::Genre { genre } => rows(
            conn,
            "t.genre = ?1 COLLATE NOCASE",
            with_navidrome,
            [genre.as_str()],
            push,
        )?,
    }
    // Only favorites are padded: a genre Flow stays in its genre however small, and the
    // library already holds everything.
    if *variant == FlowVariant::Favorites && found.len() < PAD_TARGET {
        pad(conn, &mut found, with_navidrome)?;
    }
    Ok(found.into_iter().map(|(candidate, _)| candidate).collect())
}

fn pad(conn: &Connection, found: &mut Vec<Row>, with_navidrome: bool) -> anyhow::Result<()> {
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
            format!("t.artist IN ({})", placeholders(artists.len())),
            artists,
        ),
        (
            format!("t.genre COLLATE NOCASE IN ({})", placeholders(genres.len())),
            genres,
        ),
    ];
    for (filter, values) in &passes {
        if found.len() >= PAD_TARGET || values.is_empty() {
            continue;
        }
        rows(
            conn,
            filter,
            with_navidrome,
            params_from_iter(values),
            |row| {
                if seen.insert(row.0.id.clone()) {
                    found.push(row);
                }
                found.len() < PAD_TARGET
            },
        )?;
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

    /// A Navidrome track (`navidrome:<id>`) on the album `navidrome:<album>`.
    fn add_navidrome(conn: &Connection, id: &str, artist: &str, album: &str, genre: Option<&str>) {
        conn.execute(
            "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id,
                duration_ms, format, genre)
             VALUES ('navidrome:' || ?1, 'navidrome', ?1, ?2, ?2, ?3, 'navidrome:' || ?3,
                1000, 'mp3', ?4)",
            params![id, artist, album, genre],
        )
        .unwrap();
    }

    /// The Navidrome album `navidrome:<id>` by the server artist `navidrome:<artist_id>`.
    fn navidrome_album(conn: &Connection, id: &str, artist: &str, artist_id: &str) {
        conn.execute(
            "INSERT INTO albums (id, source, title, artist, artist_id)
             VALUES ('navidrome:' || ?1, 'navidrome', ?1, ?2, 'navidrome:' || ?3)",
            params![id, artist, artist_id],
        )
        .unwrap();
    }

    #[test]
    fn navidrome_favorites_join_the_pool() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        add(&conn, "local", "Nobody", "Nobody", "nowhere", None);
        // A favorite track.
        navidrome_album(&conn, "al-1", "Tricky", "ar-1");
        add_navidrome(&conn, "s1", "Tricky", "al-1", None);
        add_navidrome(&conn, "s1b", "Tricky Kid", "al-1", None);
        favorite(&conn, "track", "navidrome:s1");
        // A favorite album.
        navidrome_album(&conn, "al-2", "Portishead", "ar-2");
        add_navidrome(&conn, "s2", "Portishead", "al-2", None);
        add_navidrome(&conn, "s3", "Portishead", "al-2", None);
        favorite(&conn, "album", "navidrome:al-2");
        // A favorite server artist: every track of its albums, whatever the track artist.
        navidrome_album(&conn, "al-3", "Massive Attack", "ar-3");
        add_navidrome(&conn, "s4", "Massive Attack", "al-3", None);
        add_navidrome(
            &conn,
            "s5",
            "Massive Attack feat. Horace Andy",
            "al-3",
            None,
        );
        favorite(&conn, "artist", "navidrome:ar-3");
        // Another server artist, not a favorite.
        navidrome_album(&conn, "al-4", "Morcheeba", "ar-4");
        add_navidrome(&conn, "s6", "Morcheeba", "al-4", None);
        let pool = pool(&conn, &FlowVariant::Favorites, true).unwrap();
        assert_eq!(
            ids(&pool),
            vec![
                "navidrome:s1",
                "navidrome:s2",
                "navidrome:s3",
                "navidrome:s4",
                "navidrome:s5"
            ]
        );
    }

    #[test]
    fn offline_navidrome_tracks_leave_the_pool() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        add(&conn, "l1", "A", "A", "la", Some("Rock"));
        add(&conn, "l2", "B", "B", "lb", Some("rock"));
        navidrome_album(&conn, "al-1", "A", "ar-1");
        add_navidrome(&conn, "n1", "A", "al-1", Some("Rock"));
        navidrome_album(&conn, "al-2", "C", "ar-2");
        add_navidrome(&conn, "n2", "C", "al-2", Some("Rock"));
        add_navidrome(&conn, "n3", "B", "al-2", Some("Rock"));
        favorite(&conn, "track", "l1");
        favorite(&conn, "track", "navidrome:n2");
        favorite(&conn, "artist", "navidrome:ar-1");
        let rock = FlowVariant::Genre {
            genre: "ROCK".into(),
        };
        let everything = vec!["l1", "l2", "navidrome:n1", "navidrome:n2", "navidrome:n3"];
        // Online: the favorites are l1, n2 and n1 (by the server artist); the genre padding
        // adds l2 and n3.
        for variant in [FlowVariant::Favorites, FlowVariant::Library, rock.clone()] {
            let online = pool(&conn, &variant, true).unwrap();
            assert_eq!(ids(&online), everything, "{variant:?} online");
        }
        // Offline: neither the variants nor the padding deal a Navidrome track, although the
        // favorites name some and the padding would match others.
        for variant in [FlowVariant::Favorites, FlowVariant::Library, rock] {
            let offline = pool(&conn, &variant, false).unwrap();
            assert_eq!(ids(&offline), vec!["l1", "l2"], "{variant:?} offline");
        }
        // Only Navidrome favorites: offline the favorites pool is empty (start falls back).
        conn.execute("DELETE FROM favorites WHERE item_id = 'l1'", [])
            .unwrap();
        assert!(pool(&conn, &FlowVariant::Favorites, false)
            .unwrap()
            .is_empty());
        let (variant, fallback) = deal_pool(&conn, &FlowVariant::Favorites, false).unwrap();
        assert_eq!(variant, FlowVariant::Library);
        assert_eq!(ids(&fallback), vec!["l1", "l2"]);
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
        let pool = pool(&conn, &FlowVariant::Favorites, true).unwrap();
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
        let pool = pool(&conn, &FlowVariant::Favorites, true).unwrap();
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
        let pool = pool(&conn, &FlowVariant::Favorites, true).unwrap();
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
            true,
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
            true,
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

        use crate::settings::RepeatMode;

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
                with_navidrome: true,
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
                repeat: RepeatMode::Off,
                shuffle: false,
            }
        }

        /// Runs `refill` online and returns (still the session, how many ids were appended).
        fn step(session: &mut Session, state: &PlayerState) -> (bool, usize) {
            let (alive, appended) = step_with(session, state, true, |_| {
                panic!("the online flag did not change")
            });
            (alive, appended.len())
        }

        /// Runs `refill` with the online flag `online`; returns (still the session, the ids
        /// appended).
        fn step_with(
            session: &mut Session,
            state: &PlayerState,
            online: bool,
            repool: impl FnOnce(bool) -> anyhow::Result<Vec<Candidate>>,
        ) -> (bool, Vec<String>) {
            let mut appended = Vec::new();
            let alive = refill(session, state, online, repool, |ids| {
                appended = ids;
                Ok(())
            });
            (alive, appended)
        }

        #[test]
        fn refill_follows_the_online_flag() {
            let db = Db::open_in_memory().unwrap();
            let conn = db.0.lock().unwrap();
            for i in 0..20 {
                let artist = format!("Artist {}", i % 4);
                conn.execute(
                    "INSERT INTO tracks (id, source, title, artist, album_artist, album,
                        album_id, duration_ms, format)
                     VALUES (?1, ?2, ?1, ?3, ?3, 'Album', ?3, 1000, 'flac')",
                    rusqlite::params![
                        if i < 10 {
                            format!("l{i}")
                        } else {
                            format!("navidrome:n{i}")
                        },
                        if i < 10 { "local" } else { "navidrome" },
                        artist
                    ],
                )
                .unwrap();
            }
            let navidrome =
                |ids: &[String]| ids.iter().filter(|id| id.starts_with("navidrome:")).count();
            let mut s = session();
            s.deck = Deck::new(
                pool(&conn, &FlowVariant::Library, true).unwrap(),
                0,
                StdRng::seed_from_u64(5),
            );
            let mut repooled = Vec::new();
            let mut repool = |include: bool| {
                repooled.push(include);
                pool(&conn, &FlowVariant::Library, include)
            };
            assert_eq!(step(&mut s, &state(5, 0, 50)).1, 0);
            // The server went away: the refill deals local tracks only.
            let (alive, batch) = step_with(&mut s, &state(5, 45, 50), false, &mut repool);
            assert!(alive);
            assert_eq!(batch.len(), FLOW_BATCH);
            assert_eq!(navidrome(&batch), 0, "{batch:?}");
            // Still offline: the same reduced pool, read once.
            let (_, batch) = step_with(&mut s, &state(6, 95, 100), false, &mut repool);
            assert_eq!(navidrome(&batch), 0, "{batch:?}");
            // Back online: Navidrome tracks are dealt again.
            let (_, batch) = step_with(&mut s, &state(7, 145, 150), true, &mut repool);
            assert_eq!(batch.len(), FLOW_BATCH);
            assert!(navidrome(&batch) > 0, "{batch:?}");
            assert_eq!(repooled, vec![false, true]);
            // A read failure keeps the old deck and tries again at the next refill; offline it
            // still deals no Navidrome track.
            let (_, batch) = step_with(&mut s, &state(8, 195, 200), false, |_| {
                Err(anyhow::anyhow!("database is locked"))
            });
            assert!(!batch.is_empty());
            assert_eq!(navidrome(&batch), 0, "{batch:?}");
            assert!(s.with_navidrome);
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
        let pool = pool(&conn, &FlowVariant::Library, true).unwrap();
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
            rig_with(db, Arc::new(AtomicBool::new(true)))
        }

        fn rig_with(db: Arc<Db>, navidrome_online: Arc<AtomicBool>) -> Rig {
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
            let (flow, sink) = FlowService::spawn(db, player.handle(), navidrome_online);
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
        fn a_flow_drops_navidrome_while_offline_and_takes_it_back() {
            // 60 local tracks and 60 Navidrome tracks, the Navidrome ones all favorites.
            let db = library(60);
            {
                let conn = db.0.lock().unwrap();
                for i in 0..60 {
                    conn.execute(
                        "INSERT INTO tracks (id, source, title, artist, album_artist, album,
                            album_id, duration_ms, format)
                         VALUES (?1, 'navidrome', ?1, ?2, ?2, 'Album', 'navidrome:al', ?3,
                            'mp3')",
                        params![
                            format!("navidrome:n{i:03}"),
                            format!("Server {}", i % 12),
                            TONE_MS as i64
                        ],
                    )
                    .unwrap();
                }
                conn.execute(
                    "INSERT INTO favorites (kind, item_id, source, added_at)
                     VALUES ('album', 'navidrome:al', 'navidrome', 1)",
                    [],
                )
                .unwrap();
            }
            let online = Arc::new(AtomicBool::new(false));
            let rig = rig_with(db, Arc::clone(&online));
            let is_navidrome = |id: &String| id.starts_with("navidrome:");
            // Offline, the Navidrome-only favorites are empty: the library without them plays.
            let outcome = rig.flow.start(FlowVariant::Favorites).unwrap();
            assert_eq!(
                outcome,
                FlowOutcome::Started {
                    variant: FlowVariant::Library
                }
            );
            // Paused while the flag changes, so the next refill is the one it is meant for;
            // playback then reaches the refill point on its own.
            let pause = || {
                wait_state(&rig.player, |s| s.status == PlayerStatus::Playing);
                rig.player.toggle().unwrap();
                wait_state(&rig.player, |s| s.status == PlayerStatus::Paused)
            };
            let paused = pause();
            assert_eq!(paused.queue_length as usize, FLOW_BATCH, "{paused:#?}");
            assert!(!rig.player.queue_ids().iter().any(is_navidrome));
            // The server is back: the next refill deals Navidrome tracks again.
            online.store(true, Ordering::SeqCst);
            rig.player.toggle().unwrap();
            wait_state(&rig.player, |s| s.queue_length as usize >= 2 * FLOW_BATCH);
            let queue = rig.player.queue_ids();
            assert!(!queue[..FLOW_BATCH].iter().any(is_navidrome));
            assert!(queue[FLOW_BATCH..].iter().any(is_navidrome), "{queue:?}");
            // Gone again: the batch after that is local only.
            let paused = pause();
            let dealt = paused.queue_length as usize;
            assert_eq!(dealt, 2 * FLOW_BATCH, "{paused:#?}");
            online.store(false, Ordering::SeqCst);
            rig.player.toggle().unwrap();
            wait_state(&rig.player, |s| s.queue_length as usize > dealt);
            let queue = rig.player.queue_ids();
            assert!(!queue[dealt..].iter().any(is_navidrome), "{queue:?}");
            // The Flow is still that Flow.
            assert_eq!(rig.player.state().flow, Some(FlowVariant::Library));
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
