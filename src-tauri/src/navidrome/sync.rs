//! Library sync: the server's albums and songs into `albums` / `tracks` (`source = 'navidrome'`).
//!
//! A full pass lists every album (`getAlbumList2`) and every song (`search3` with an empty
//! query, or `getAlbum` per album when that returns nothing). Network calls run outside the
//! database lock; each page is written in its own short transaction, so a first sync of a large
//! library never blocks the UI's reads for long. Rows the server no longer lists are deleted
//! only after both listings completed.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;

use super::client::{AlbumId3, Child, NavidromeError, ReplayGainInfo, SubsonicClient, PAGE};
use crate::db::Db;
use crate::library::model::{navidrome_cover_id, navidrome_id, Album, Source, Track};
use crate::library::scan::lock;

/// Hard cap on the pages of one paged listing: 2 000 pages of 500 are a million items.
const MAX_PAGES: u32 = 2_000;

/// A full pass runs at least this often, even when `lastModified` did not change.
const FULL_PASS_EVERY_MS: i64 = 24 * 60 * 60 * 1000;

/// `changed()` fires at most this often while a pass writes.
const CHANGED_EVERY: Duration = Duration::from_secs(2);

const KEY_SERVER: &str = "server";
const KEY_LAST_MODIFIED: &str = "last_modified";
const KEY_LAST_FULL_SYNC: &str = "last_full_sync";
const KEY_LAST_SYNC: &str = "last_sync";

const TRACK_COLUMNS: &[&str] = &[
    "id",
    "source",
    "title",
    "artist",
    "album_artist",
    "album",
    "album_id",
    "track_no",
    "disc_no",
    "duration_ms",
    "year",
    "genre",
    "format",
    "sample_rate",
    "bit_depth",
    "bitrate_kbps",
    "cover_id",
    "path",
    "replay_gain_track_db",
    "replay_gain_album_db",
    "replay_gain_track_peak",
    "replay_gain_album_peak",
    "folder",
    "mtime",
    "size",
];

const ALBUM_COLUMNS: &[&str] = &[
    "id",
    "source",
    "title",
    "artist",
    "year",
    "cover_id",
    "track_count",
    "duration_ms",
    "artist_id",
];

/// An upsert keyed on `id` that only updates a row when a mapped column differs, so an
/// unchanged row is not rewritten (no FTS churn, no WAL growth). Never `INSERT OR REPLACE`:
/// that bypasses the FTS delete trigger.
fn upsert_sql(table: &str, columns: &[&str]) -> String {
    let values: Vec<String> = (1..=columns.len()).map(|i| format!("?{i}")).collect();
    let rest = &columns[1..];
    let set: Vec<String> = rest.iter().map(|c| format!("{c} = excluded.{c}")).collect();
    let differs: Vec<String> = rest
        .iter()
        .map(|c| format!("{table}.{c} IS NOT excluded.{c}"))
        .collect();
    format!(
        "INSERT INTO {table} ({}) VALUES ({}) ON CONFLICT(id) DO UPDATE SET {} WHERE {}",
        columns.join(", "),
        values.join(", "),
        set.join(", "),
        differs.join(" OR ")
    )
}

static TRACK_UPSERT: LazyLock<String> = LazyLock::new(|| upsert_sql("tracks", TRACK_COLUMNS));
static ALBUM_UPSERT: LazyLock<String> = LazyLock::new(|| upsert_sql("albums", ALBUM_COLUMNS));

/// How a paged listing pages. Tests shrink it.
#[derive(Clone, Copy, Debug)]
struct Limits {
    page: u32,
    max_pages: u32,
}

const LIMITS: Limits = Limits {
    page: PAGE,
    max_pages: MAX_PAGES,
};

/// The key of a server account: the lowercased URL and the lowercased user.
pub fn server_key(url: &str, user: &str) -> String {
    // Lowercased like the keychain target: retyping the user's case is no server change.
    format!("{}\u{1f}{}", url.to_lowercase(), user.to_lowercase())
}

fn state(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM navidrome_state WHERE key = ?1",
        [key],
        |row| row.get(0),
    )
    .optional()
}

fn set_state(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO navidrome_state (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Wipes every Navidrome row when the stored server key differs from `key` (or none is stored
/// yet), then stores `key`. Returns whether it wiped.
pub fn ensure_server(db: &Db, key: &str) -> anyhow::Result<bool> {
    let mut conn = lock(db)?;
    if state(&conn, KEY_SERVER)?.as_deref() == Some(key) {
        return Ok(false);
    }
    let tx = conn.transaction()?;
    wipe_navidrome(&tx)?;
    set_state(&tx, KEY_SERVER, key)?;
    tx.commit()?;
    Ok(true)
}

/// Deletes every Navidrome row (tracks, albums, favorites, play stats, both queues, remote
/// playlists, cover rows and cover lookups) and every sync state key except `server`. The
/// cached cover files go after the commit, through the service's cover purge.
pub fn wipe_navidrome(tx: &Transaction) -> anyhow::Result<()> {
    tx.execute_batch(
        "DELETE FROM tracks WHERE source = 'navidrome';
         DELETE FROM albums WHERE source = 'navidrome';
         DELETE FROM covers WHERE id GLOB 'nd-*';
         DELETE FROM cover_lookups WHERE album_id GLOB 'navidrome:*';
         DELETE FROM favorites WHERE source = 'navidrome' OR item_id GLOB 'navidrome:*';
         DELETE FROM track_stats WHERE track_id GLOB 'navidrome:*';
         DELETE FROM navidrome_star_queue;
         DELETE FROM navidrome_scrobble_queue;
         DELETE FROM remote_playlist_tracks;
         DELETE FROM remote_playlists;
         DELETE FROM navidrome_state WHERE key <> 'server';",
    )?;
    Ok(())
}

/// Songs written so far in the song pass; `total` is the sum of the albums' song counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProgress {
    pub done: u32,
    pub total: Option<u32>,
}

/// What one sync did. `tracks` and `albums` count the Navidrome rows of the index afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncReport {
    pub full_pass: bool,
    pub tracks: u32,
    pub albums: u32,
    pub changed: bool,
}

#[derive(Debug)]
pub enum SyncError {
    Api(NavidromeError),
    Db(anyhow::Error),
    /// The pass was refused as incomplete or implausible; nothing was deleted.
    Refused(String),
    /// The caller cancelled the pass (the account changed or the app quits). Nothing was
    /// written after the cancel was seen.
    Cancelled,
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncError::Api(err) => write!(f, "{err}"),
            SyncError::Db(err) => write!(f, "library database: {err:#}"),
            SyncError::Refused(reason) => write!(f, "sync stopped: {reason}"),
            SyncError::Cancelled => f.write_str("sync cancelled"),
        }
    }
}

impl std::error::Error for SyncError {}

impl From<NavidromeError> for SyncError {
    fn from(err: NavidromeError) -> SyncError {
        SyncError::Api(err)
    }
}

impl From<anyhow::Error> for SyncError {
    fn from(err: anyhow::Error) -> SyncError {
        SyncError::Db(err)
    }
}

impl From<rusqlite::Error> for SyncError {
    fn from(err: rusqlite::Error) -> SyncError {
        SyncError::Db(err.into())
    }
}

/// Writes pages in their own transactions and fires `changed` after the first page that
/// changed rows, then at most every [`CHANGED_EVERY`].
struct Writer<'a> {
    db: &'a Db,
    changed: &'a dyn Fn(),
    cancelled: &'a dyn Fn() -> bool,
    notified: Option<Instant>,
    any: bool,
}

impl Writer<'_> {
    /// Runs `write` in one transaction under the lock; it returns the rows it changed.
    ///
    /// The cancel check runs under the database lock: a caller that sets the cancel and then
    /// takes the lock (to wipe the index) knows that no write of this pass follows its own.
    fn write(
        &mut self,
        write: impl FnOnce(&Transaction) -> Result<usize, SyncError>,
    ) -> Result<(), SyncError> {
        let rows = {
            let mut conn = lock(self.db)?;
            if (self.cancelled)() {
                return Err(SyncError::Cancelled);
            }
            let tx = conn.transaction()?;
            let rows = write(&tx)?;
            tx.commit()?;
            rows
        };
        if rows > 0 {
            self.any = true;
            if self.notified.is_none_or(|at| at.elapsed() >= CHANGED_EVERY) {
                self.notified = Some(Instant::now());
                (self.changed)();
            }
        }
        Ok(())
    }
}

/// Pages through `fetch(offset, size)` until a short page and hands each page's new items (ids
/// not seen before in this listing) to `write`. Returns how many distinct items it saw.
///
/// A server that ignores the offset would page forever: a full page without a new id stops
/// the listing, and so does the page cap. Both refuse the pass, so it deletes nothing.
fn page_through<T>(
    limits: Limits,
    what: &str,
    mut fetch: impl FnMut(u32, u32) -> Result<Vec<T>, NavidromeError>,
    id: impl Fn(&T) -> &str,
    mut write: impl FnMut(Vec<T>) -> Result<(), SyncError>,
) -> Result<u32, SyncError> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut offset: u32 = 0;
    for _ in 0..limits.max_pages {
        let page = fetch(offset, limits.page)?;
        let len = page.len() as u32;
        let fresh: Vec<T> = page
            .into_iter()
            .filter(|item| {
                let id = id(item);
                !id.is_empty() && seen.insert(id.to_owned())
            })
            .collect();
        if len > 0 && fresh.is_empty() {
            return Err(SyncError::Refused(format!(
                "the server repeated a page of {what}"
            )));
        }
        if !fresh.is_empty() {
            write(fresh)?;
        }
        if len < limits.page {
            return Ok(seen.len() as u32);
        }
        offset = offset.saturating_add(len);
    }
    Err(SyncError::Refused(format!(
        "more than {} pages of {what}",
        limits.max_pages
    )))
}

fn reset_seen(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS nd_seen_tracks (id TEXT PRIMARY KEY);
         CREATE TEMP TABLE IF NOT EXISTS nd_seen_albums (id TEXT PRIMARY KEY);
         DELETE FROM temp.nd_seen_tracks;
         DELETE FROM temp.nd_seen_albums;",
    )
}

/// Upserts a track and marks it seen; returns the rows changed (0 when unchanged).
fn write_track(tx: &Transaction, t: &Track) -> rusqlite::Result<usize> {
    let rows = tx.prepare_cached(&TRACK_UPSERT)?.execute(params![
        t.id,
        t.source.as_str(),
        t.title,
        t.artist,
        t.album_artist,
        t.album,
        t.album_id,
        t.track_no.map(i64::from),
        t.disc_no.map(i64::from),
        t.duration_ms as i64,
        t.year,
        t.genre,
        t.format,
        t.sample_rate.map(i64::from),
        t.bit_depth.map(i64::from),
        t.bitrate_kbps.map(i64::from),
        t.cover_id,
        t.path,
        t.replay_gain_track_db.map(f64::from),
        t.replay_gain_album_db.map(f64::from),
        t.replay_gain_track_peak.map(f64::from),
        t.replay_gain_album_peak.map(f64::from),
        None::<String>,
        None::<i64>,
        None::<i64>,
    ])?;
    tx.prepare_cached("INSERT OR IGNORE INTO temp.nd_seen_tracks (id) VALUES (?1)")?
        .execute([&t.id])?;
    Ok(rows)
}

/// Upserts an album (always with the server's artist id) and marks it seen.
fn write_album(tx: &Transaction, album: &AlbumId3) -> rusqlite::Result<usize> {
    let (row, artist_id) = album_row(album);
    let rows = tx.prepare_cached(&ALBUM_UPSERT)?.execute(params![
        row.id,
        row.source.as_str(),
        row.title,
        row.artist,
        row.year,
        row.cover_id,
        i64::from(row.track_count),
        row.duration_ms as i64,
        artist_id,
    ])?;
    tx.prepare_cached("INSERT OR IGNORE INTO temp.nd_seen_albums (id) VALUES (?1)")?
        .execute([&row.id])?;
    Ok(rows)
}

fn counts(conn: &Connection) -> rusqlite::Result<(u32, u32)> {
    let count = |sql: &str| conn.query_row(sql, [], |row| row.get::<_, u32>(0));
    Ok((
        count("SELECT COUNT(*) FROM tracks WHERE source = 'navidrome'")?,
        count("SELECT COUNT(*) FROM albums WHERE source = 'navidrome'")?,
    ))
}

pub fn sync_library(
    db: &Db,
    client: &SubsonicClient,
    now_ms: i64,
    progress: &dyn Fn(SyncProgress),
    changed: &dyn Fn(),
) -> Result<SyncReport, SyncError> {
    sync_library_cancellable(db, client, now_ms, progress, changed, &|| false)
}

/// [`sync_library`] that stops with [`SyncError::Cancelled`] once `cancelled` returns true.
/// `cancelled` is checked under the database lock before every write.
pub fn sync_library_cancellable(
    db: &Db,
    client: &SubsonicClient,
    now_ms: i64,
    progress: &dyn Fn(SyncProgress),
    changed: &dyn Fn(),
    cancelled: &dyn Fn() -> bool,
) -> Result<SyncReport, SyncError> {
    sync_with(db, client, now_ms, progress, changed, cancelled, LIMITS)
}

/// The songs listed must reach at least this share of the albums' song counts, or the pass
/// is refused (a proxy or server that cut the listing short).
const MIN_SONG_SHARE_PERCENT: u64 = 50;

fn sync_with(
    db: &Db,
    client: &SubsonicClient,
    now_ms: i64,
    progress: &dyn Fn(SyncProgress),
    changed: &dyn Fn(),
    cancelled: &dyn Fn() -> bool,
    limits: Limits,
) -> Result<SyncReport, SyncError> {
    let remote = client.indexes_last_modified()?.to_string();
    let (stored, last_full) = {
        let conn = lock(db)?;
        (
            state(&conn, KEY_LAST_MODIFIED)?,
            state(&conn, KEY_LAST_FULL_SYNC)?.and_then(|v| v.parse::<i64>().ok()),
        )
    };
    let due = last_full.is_none_or(|at| now_ms.saturating_sub(at) >= FULL_PASS_EVERY_MS);
    // An incomplete pass never stores lastModified, so it differs here and the pass repeats.
    if stored.as_deref() == Some(remote.as_str()) && !due {
        let conn = lock(db)?;
        if cancelled() {
            return Err(SyncError::Cancelled);
        }
        set_state(&conn, KEY_LAST_SYNC, &now_ms.to_string())?;
        let (tracks, albums) = counts(&conn)?;
        return Ok(SyncReport {
            full_pass: false,
            tracks,
            albums,
            changed: false,
        });
    }

    reset_seen(&*lock(db)?)?;
    let mut writer = Writer {
        db,
        changed,
        cancelled,
        notified: None,
        any: false,
    };

    // Albums: their cover and song count, in listing order for the getAlbum fallback.
    let mut covers: HashMap<String, Option<String>> = HashMap::new();
    let mut album_ids: Vec<String> = Vec::new();
    let mut total: u32 = 0;
    page_through(
        limits,
        "albums",
        |offset, size| client.album_list2(offset, size),
        |album| album.id.as_str(),
        |albums| {
            for album in &albums {
                covers.insert(album.id.clone(), album.cover_art.clone());
                album_ids.push(album.id.clone());
                total = total.saturating_add(album.song_count);
            }
            writer.write(|tx| {
                let mut rows = 0;
                for album in &albums {
                    rows += write_album(tx, album)?;
                }
                Ok(rows)
            })
        },
    )?;
    let total = (total > 0).then_some(total);
    let album_cover = |song: &Child| -> Option<String> {
        song.album_id
            .as_ref()
            .and_then(|id| covers.get(id))
            .cloned()
            .flatten()
    };

    // Songs.
    let mut done: u32 = 0;
    progress(SyncProgress { done, total });
    let songs = page_through(
        limits,
        "songs",
        |offset, count| client.search3_songs(offset, count),
        |song| song.id.as_str(),
        |songs| {
            writer.write(|tx| {
                let mut rows = 0;
                for song in &songs {
                    rows += write_track(tx, &song_row(song, album_cover(song).as_deref()))?;
                }
                Ok(rows)
            })?;
            done = done.saturating_add(songs.len() as u32);
            progress(SyncProgress { done, total });
            Ok(())
        },
    )?;
    if songs == 0 {
        // Not every server answers search3 with an empty query: the songs album by album.
        let mut seen: HashSet<String> = HashSet::new();
        for id in &album_ids {
            let (album, songs) = client.album(id)?;
            let fresh: Vec<Child> = songs
                .into_iter()
                .filter(|s| !s.id.is_empty() && seen.insert(s.id.clone()))
                .collect();
            let cover = album
                .cover_art
                .clone()
                .or_else(|| covers.get(id).cloned().flatten());
            writer.write(|tx| {
                let mut rows = 0;
                if album.id == *id {
                    rows += write_album(tx, &album)?;
                }
                for song in &fresh {
                    rows += write_track(tx, &song_row(song, cover.as_deref()))?;
                }
                Ok(rows)
            })?;
            done = done.saturating_add(fresh.len() as u32);
            progress(SyncProgress { done, total });
        }
    }

    // Both listings completed: delete what the server no longer lists.
    writer.write(|tx| {
        let seen: i64 = tx.query_row("SELECT COUNT(*) FROM temp.nd_seen_tracks", [], |row| {
            row.get(0)
        })?;
        let indexed: i64 = tx.query_row(
            "SELECT COUNT(*) FROM tracks WHERE source = 'navidrome'",
            [],
            |row| row.get(0),
        )?;
        if seen == 0 && indexed > 0 {
            return Err(SyncError::Refused(
                "the server listed no songs; the index is kept".into(),
            ));
        }
        if let Some(total) = total {
            if (seen.max(0) as u64) * 100 < u64::from(total) * MIN_SONG_SHARE_PERCENT {
                return Err(SyncError::Refused(format!(
                    "the server listed {seen} of {total} songs; the index is kept"
                )));
            }
        }
        let mut rows = tx.execute(
            "DELETE FROM tracks WHERE source = 'navidrome'
             AND id NOT IN (SELECT id FROM temp.nd_seen_tracks)",
            [],
        )?;
        rows += tx.execute(
            "DELETE FROM albums WHERE source = 'navidrome'
             AND id NOT IN (SELECT id FROM temp.nd_seen_albums)",
            [],
        )?;
        let now = now_ms.to_string();
        set_state(tx, KEY_LAST_MODIFIED, &remote)?;
        set_state(tx, KEY_LAST_FULL_SYNC, &now)?;
        set_state(tx, KEY_LAST_SYNC, &now)?;
        tx.execute_batch("DELETE FROM temp.nd_seen_tracks; DELETE FROM temp.nd_seen_albums;")?;
        Ok(rows)
    })?;

    if writer.any {
        changed();
    }
    let (tracks, albums) = counts(&*lock(db)?)?;
    Ok(SyncReport {
        full_pass: true,
        tracks,
        albums,
        changed: writer.any,
    })
}

/// A trimmed, non-empty text.
fn text(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn positive(value: Option<u32>) -> Option<u32> {
    value.filter(|v| *v > 0)
}

/// The track row of a song. `album_cover` is the album's `coverArt` when the album is known;
/// it wins over the song's own, so one cover is fetched per album.
pub fn song_row(song: &Child, album_cover: Option<&str>) -> Track {
    let artist = song.artist.clone().unwrap_or_default();
    let album_artist = text(&song.display_album_artist)
        .or_else(|| {
            song.album_artists
                .first()
                .map(|a| a.name.trim())
                .filter(|n| !n.is_empty())
        })
        .map(str::to_owned)
        .unwrap_or_else(|| artist.clone());
    let format = text(&song.suffix)
        .map(str::to_lowercase)
        .or_else(|| {
            text(&song.content_type)
                .and_then(|ct| ct.split(';').next())
                .and_then(|ct| ct.split_once('/'))
                .map(|(_, sub)| sub.trim().to_lowercase())
                .filter(|sub| !sub.is_empty())
        })
        .unwrap_or_default();
    let gain = |pick: fn(&ReplayGainInfo) -> Option<f64>| {
        song.replay_gain.as_ref().and_then(pick).map(|v| v as f32)
    };
    Track {
        id: navidrome_id(&song.id),
        source: Source::Navidrome,
        title: song.title.clone(),
        artist,
        album_artist,
        album: song.album.clone().unwrap_or_default(),
        album_id: text(&song.album_id).map(navidrome_id).unwrap_or_default(),
        track_no: positive(song.track),
        disc_no: positive(song.disc_number),
        duration_ms: u64::from(song.duration.unwrap_or(0)) * 1000,
        year: positive(song.year).and_then(|y| i32::try_from(y).ok()),
        genre: text(&song.genre).map(str::to_owned),
        format,
        sample_rate: positive(song.sampling_rate),
        bit_depth: positive(song.bit_depth).and_then(|d| u8::try_from(d).ok()),
        bitrate_kbps: positive(song.bit_rate),
        cover_id: album_cover
            .and_then(navidrome_cover_id)
            .or_else(|| song.cover_art.as_deref().and_then(navidrome_cover_id)),
        path: None,
        replay_gain_track_db: gain(|g| g.track_gain),
        replay_gain_album_db: gain(|g| g.album_gain),
        replay_gain_track_peak: gain(|g| g.track_peak),
        replay_gain_album_peak: gain(|g| g.album_peak),
    }
}

/// The album row of an album and its `navidrome:` artist id.
pub fn album_row(album: &AlbumId3) -> (Album, Option<String>) {
    (
        Album {
            id: navidrome_id(&album.id),
            source: Source::Navidrome,
            title: album.name.clone(),
            artist: album.artist.clone().unwrap_or_default(),
            year: positive(album.year).and_then(|y| i32::try_from(y).ok()),
            cover_id: album.cover_art.as_deref().and_then(navidrome_cover_id),
            track_count: album.song_count,
            duration_ms: u64::from(album.duration) * 1000,
        },
        text(&album.artist_id).map(navidrome_id),
    )
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;

    use rusqlite::OptionalExtension;

    use super::*;
    use crate::library::model::Source;
    use crate::library::query;
    use crate::navidrome::client::ReplayGainInfo;
    use crate::navidrome::testserver::{FakeLibrary, FakeServer, Fault};

    /// A fixed "now" (ms).
    const NOW: i64 = 1_800_000_000_000;
    const DAY_MS: i64 = 24 * 60 * 60 * 1000;

    struct Env {
        server: FakeServer,
        client: SubsonicClient,
        db: Db,
    }

    fn env(lib: FakeLibrary) -> Env {
        let server = FakeServer::start(lib);
        let client = SubsonicClient::new(server.account()).unwrap();
        let db = Db::open_in_memory().unwrap();
        Env { server, client, db }
    }

    impl Env {
        fn sync(&self) -> Result<SyncReport, SyncError> {
            self.sync_at(NOW)
        }

        fn sync_at(&self, now: i64) -> Result<SyncReport, SyncError> {
            sync_library(&self.db, &self.client, now, &|_| {}, &|| {})
        }

        fn count(&self, sql: &str) -> i64 {
            let conn = self.db.0.lock().unwrap();
            conn.query_row(sql, [], |row| row.get(0)).unwrap()
        }

        fn state(&self, key: &str) -> Option<String> {
            let conn = self.db.0.lock().unwrap();
            conn.query_row(
                "SELECT value FROM navidrome_state WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .unwrap()
        }

        fn search3_requests(&self) -> usize {
            self.server
                .requests()
                .iter()
                .filter(|r| r.method == "search3")
                .count()
        }

        fn nd_tracks(&self) -> i64 {
            self.count("SELECT COUNT(*) FROM tracks WHERE source = 'navidrome'")
        }

        fn nd_albums(&self) -> i64 {
            self.count("SELECT COUNT(*) FROM albums WHERE source = 'navidrome'")
        }

        fn title(&self, id: &str) -> Option<String> {
            let conn = self.db.0.lock().unwrap();
            conn.query_row("SELECT title FROM tracks WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()
            .unwrap()
        }

        fn insert_local(&self) {
            let conn = self.db.0.lock().unwrap();
            conn.execute(
                "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id,
                    duration_ms, format, path, folder)
                 VALUES ('local:t1', 'local', 'Local Song', 'Local Band', 'Local Band',
                    'Local Album', 'local:a1', 1000, 'flac', '/m/a.flac', '/m')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO albums (id, source, title, artist, track_count, duration_ms)
                 VALUES ('local:a1', 'local', 'Local Album', 'Local Band', 1, 1000)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO favorites (kind, item_id, source, added_at)
                 VALUES ('track', 'local:t1', 'local', 1), ('artist', 'Local Band', 'local', 1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO track_stats (track_id, play_count) VALUES ('local:t1', 3)",
                [],
            )
            .unwrap();
        }

        fn local_rows(&self) -> (i64, i64, i64, i64) {
            (
                self.count("SELECT COUNT(*) FROM tracks WHERE source = 'local'"),
                self.count("SELECT COUNT(*) FROM albums WHERE source = 'local'"),
                self.count("SELECT COUNT(*) FROM favorites WHERE source = 'local'"),
                self.count("SELECT COUNT(*) FROM track_stats WHERE track_id = 'local:t1'"),
            )
        }
    }

    /// Removes album `al-<n>` and its songs.
    fn remove_album(lib: &mut FakeLibrary, n: usize) {
        let id = format!("al-{n}");
        lib.albums.retain(|a| a.id != id);
        lib.songs
            .retain(|s| s.album_id.as_deref() != Some(id.as_str()));
    }

    #[test]
    fn first_sync_indexes_everything() {
        let env = env(FakeLibrary::synthetic(20, 12));
        let fired = Cell::new(0);
        let reports = RefCell::new(Vec::new());
        let report = sync_library(
            &env.db,
            &env.client,
            NOW,
            &|p| reports.borrow_mut().push(p),
            &|| fired.set(fired.get() + 1),
        )
        .unwrap();
        assert_eq!(
            report,
            SyncReport {
                full_pass: true,
                tracks: 240,
                albums: 20,
                changed: true,
            }
        );
        assert_eq!(env.nd_tracks(), 240);
        assert_eq!(env.nd_albums(), 20);
        assert_eq!(
            env.count(
                "SELECT COUNT(*) FROM albums WHERE source = 'navidrome'
                 AND artist_id GLOB 'navidrome:ar-*'"
            ),
            20
        );
        assert_eq!(
            env.count("SELECT COUNT(*) FROM tracks WHERE id NOT GLOB 'navidrome:tr-*'"),
            0
        );
        let conn = env.db.0.lock().unwrap();
        let found = query::search(&conn, "Demo Song 17", 50).unwrap();
        assert!(
            found.tracks.iter().any(|t| t.id == "navidrome:tr-16"),
            "{found:?}"
        );
        assert_eq!(
            query::track_count(&conn, Some(Source::Navidrome)).unwrap(),
            240
        );
        drop(conn);
        assert!(fired.get() >= 1);
        let last = *reports.borrow().last().unwrap();
        assert_eq!(
            last,
            SyncProgress {
                done: 240,
                total: Some(240)
            }
        );
        assert_eq!(env.state("last_modified").as_deref(), Some("1700000000000"));
        assert_eq!(env.state("last_full_sync"), Some(NOW.to_string()));
        assert_eq!(env.state("last_sync"), Some(NOW.to_string()));
    }

    #[test]
    fn second_sync_without_changes_skips_song_pass() {
        let env = env(FakeLibrary::synthetic(4, 3));
        env.sync().unwrap();
        let before = env.search3_requests();
        assert!(before > 0);
        let fired = Cell::new(0);
        let report = sync_library(&env.db, &env.client, NOW + 60_000, &|_| {}, &|| {
            fired.set(fired.get() + 1)
        })
        .unwrap();
        assert!(!report.full_pass);
        assert!(!report.changed);
        assert_eq!(report.tracks, 12);
        assert_eq!(env.search3_requests(), before);
        assert_eq!(fired.get(), 0);
        assert_eq!(env.state("last_sync"), Some((NOW + 60_000).to_string()));
        assert_eq!(env.state("last_full_sync"), Some(NOW.to_string()));
    }

    #[test]
    fn changed_server_updates_and_deletes() {
        let env = env(FakeLibrary::synthetic(20, 12));
        env.sync().unwrap();
        env.server.edit(|lib| {
            lib.songs[0].title = "Renamed Demo Song".into();
            remove_album(lib, 5);
        });
        let report = env.sync().unwrap();
        assert!(report.full_pass && report.changed);
        assert_eq!(
            env.title("navidrome:tr-0").as_deref(),
            Some("Renamed Demo Song")
        );
        assert_eq!(env.nd_tracks(), 228);
        assert_eq!(env.nd_albums(), 19);
        assert_eq!(
            env.count("SELECT COUNT(*) FROM tracks WHERE album_id = 'navidrome:al-5'"),
            0
        );
        assert_eq!(
            env.count("SELECT COUNT(*) FROM albums WHERE id = 'navidrome:al-5'"),
            0
        );
        let conn = env.db.0.lock().unwrap();
        let found = query::search(&conn, "Renamed", 10).unwrap();
        assert_eq!(found.tracks.len(), 1);
    }

    #[test]
    fn interrupted_pass_deletes_nothing() {
        // 1 200 songs: three search3 pages of 500.
        let env = env(FakeLibrary::synthetic(100, 12));
        env.sync().unwrap();
        assert_eq!(env.nd_tracks(), 1_200);
        let stored = env.state("last_modified");
        env.server.edit(|lib| {
            remove_album(lib, 0);
            remove_album(lib, 99);
        });
        // Cut the connection of page 2, right after page 1 was written.
        let result = sync_library(
            &env.db,
            &env.client,
            NOW,
            &|p| {
                if p.done == 500 {
                    env.server.set_fault(Fault::CutOnce {
                        method: "search3",
                        after_bytes: 10,
                    });
                }
            },
            &|| {},
        );
        match result {
            Err(SyncError::Api(err)) => assert!(err.is_offline(), "{err:?}"),
            other => panic!("expected an offline error, got {other:?}"),
        }
        assert_eq!(env.nd_tracks(), 1_200);
        assert_eq!(env.nd_albums(), 100);
        assert_eq!(env.state("last_modified"), stored);

        let report = env.sync().unwrap();
        assert!(report.full_pass);
        assert_eq!(env.nd_tracks(), 1_176);
        assert_eq!(env.nd_albums(), 98);
        assert_ne!(env.state("last_modified"), stored);
    }

    #[test]
    fn server_ignoring_offset_terminates_and_deletes_nothing() {
        let env = env(FakeLibrary::synthetic(100, 12));
        env.sync().unwrap();
        let stored = env.state("last_modified");
        let before = env.search3_requests();
        env.server.edit(|lib| remove_album(lib, 50));
        env.server.set_fault(Fault::IgnoreOffset);
        let result = env.sync();
        assert!(matches!(result, Err(SyncError::Refused(_))), "{result:?}");
        // Page 1, then page 2 repeats it: the loop stops there.
        assert_eq!(env.search3_requests() - before, 2);
        assert_eq!(env.nd_tracks(), 1_200);
        assert_eq!(env.state("last_modified"), stored);
    }

    #[test]
    fn page_cap_stops_and_deletes_nothing() {
        let env = env(FakeLibrary::synthetic(4, 6));
        env.sync().unwrap();
        env.server.edit(|lib| remove_album(lib, 3));
        let limits = Limits {
            page: 5,
            max_pages: 3,
        };
        let result = sync_with(
            &env.db,
            &env.client,
            NOW,
            &|_| {},
            &|| {},
            &|| false,
            limits,
        );
        assert!(matches!(result, Err(SyncError::Refused(_))), "{result:?}");
        assert_eq!(env.nd_tracks(), 24);
        assert_eq!(env.nd_albums(), 4);
        // Within the cap the same small pages complete.
        let limits = Limits {
            page: 5,
            max_pages: 10,
        };
        sync_with(
            &env.db,
            &env.client,
            NOW,
            &|_| {},
            &|| {},
            &|| false,
            limits,
        )
        .unwrap();
        assert_eq!(env.nd_tracks(), 18);
        assert_eq!(env.nd_albums(), 3);
    }

    #[test]
    fn search3_unsupported_falls_back_to_albums() {
        let env = env(FakeLibrary::synthetic(5, 4));
        env.server.set_fault(Fault::Search3Empty);
        let report = env.sync().unwrap();
        assert!(report.full_pass);
        assert_eq!(env.nd_tracks(), 20);
        assert_eq!(
            env.server
                .requests()
                .iter()
                .filter(|r| r.method == "getAlbum")
                .count(),
            5
        );
        // Deletions work the same way through the fallback.
        env.server.edit(|lib| remove_album(lib, 2));
        env.sync().unwrap();
        assert_eq!(env.nd_tracks(), 16);
        assert_eq!(env.nd_albums(), 4);
    }

    #[test]
    fn zero_songs_never_wipes_the_index() {
        let env = env(FakeLibrary::synthetic(3, 4));
        env.sync().unwrap();
        let stored = env.state("last_modified");
        env.server.edit(|lib| {
            lib.albums.clear();
            lib.songs.clear();
        });
        let result = env.sync();
        assert!(matches!(result, Err(SyncError::Refused(_))), "{result:?}");
        assert_eq!(env.nd_tracks(), 12);
        assert_eq!(env.nd_albums(), 3);
        assert_eq!(env.state("last_modified"), stored);

        // An empty server and an empty index is a complete pass.
        let empty = self::env(FakeLibrary::synthetic(0, 0));
        let report = empty.sync().unwrap();
        assert_eq!(report.tracks, 0);
        assert!(empty.state("last_modified").is_some());
    }

    #[test]
    fn server_change_wipes_the_index() {
        let env = env(FakeLibrary::synthetic(3, 4));
        let key = server_key(&env.server.url(), "demo");
        assert!(ensure_server(&env.db, &key).unwrap());
        assert!(!ensure_server(&env.db, &key).unwrap());
        env.insert_local();
        env.sync().unwrap();
        {
            let conn = env.db.0.lock().unwrap();
            conn.execute_batch(
                "INSERT INTO favorites (kind, item_id, source, added_at)
                     VALUES ('track', 'navidrome:tr-0', 'navidrome', 1),
                            ('artist', 'navidrome:ar-0', 'navidrome', 1);
                 INSERT INTO track_stats (track_id, play_count) VALUES ('navidrome:tr-0', 2);
                 INSERT INTO navidrome_star_queue (kind, item_id, star, queued_at)
                     VALUES ('track', 'navidrome:tr-1', 1, 1);
                 INSERT INTO navidrome_scrobble_queue (track_id, played_at)
                     VALUES ('navidrome:tr-2', 1);
                 INSERT INTO remote_playlists (id, name, updated_at)
                     VALUES ('navidrome:pl-0', 'Demo Playlist 1', 1);
                 INSERT INTO remote_playlist_tracks (playlist_id, position, track_id)
                     VALUES ('navidrome:pl-0', 0, 'navidrome:tr-0');
                 INSERT INTO covers (id, file) VALUES ('nd-al-0_cover', 'nd-al-0_cover.jpg'),
                                                      ('abc123', 'abc123.jpg');
                 INSERT INTO cover_lookups (album_id, status, checked_at)
                     VALUES ('navidrome:al-0', 'notfound', 1),
                            ('local:album', 'notfound', 1);",
            )
            .unwrap();
        }
        assert!(env.nd_tracks() > 0);

        let other = server_key(&env.server.url(), "someone-else");
        assert!(ensure_server(&env.db, &other).unwrap());
        assert_eq!(env.nd_tracks(), 0);
        assert_eq!(env.nd_albums(), 0);
        for table in [
            "favorites WHERE source = 'navidrome'",
            "track_stats WHERE track_id GLOB 'navidrome:*'",
            "navidrome_star_queue",
            "navidrome_scrobble_queue",
            "remote_playlists",
            "remote_playlist_tracks",
            "navidrome_state WHERE key <> 'server'",
            "covers WHERE id GLOB 'nd-*'",
            "cover_lookups WHERE album_id GLOB 'navidrome:*'",
        ] {
            assert_eq!(
                env.count(&format!("SELECT COUNT(*) FROM {table}")),
                0,
                "{table}"
            );
        }
        // Local cover rows and lookups stay.
        assert_eq!(
            env.count("SELECT COUNT(*) FROM covers WHERE id = 'abc123'"),
            1
        );
        assert_eq!(
            env.count("SELECT COUNT(*) FROM cover_lookups WHERE album_id = 'local:album'"),
            1
        );
        assert_eq!(env.state("server"), Some(other));
        assert_eq!(env.local_rows(), (1, 1, 2, 1));

        // The next sync is a full pass into the empty index.
        let report = env.sync().unwrap();
        assert!(report.full_pass);
        assert_eq!(env.nd_tracks(), 12);
    }

    #[test]
    fn server_key_ignores_case() {
        assert_eq!(
            server_key("https://NAS.example:4533/Music", "Demo"),
            "https://nas.example:4533/music\u{1f}demo"
        );
        assert_eq!(server_key("https://h", "a"), server_key("https://H", "A"));
        assert_ne!(server_key("https://h", "a"), server_key("https://h", "b"));
    }

    #[test]
    fn local_rows_untouched_by_sync() {
        let env = env(FakeLibrary::synthetic(3, 4));
        env.insert_local();
        env.sync().unwrap();
        assert_eq!(env.local_rows(), (1, 1, 2, 1));
        env.server.edit(|lib| remove_album(lib, 1));
        env.sync().unwrap();
        assert_eq!(env.nd_tracks(), 8);
        assert_eq!(env.local_rows(), (1, 1, 2, 1));
        assert_eq!(env.title("local:t1").as_deref(), Some("Local Song"));
    }

    #[test]
    fn unchanged_rows_are_not_rewritten() {
        let env = env(FakeLibrary::synthetic(6, 5));
        env.sync().unwrap();
        {
            let conn = env.db.0.lock().unwrap();
            conn.execute_batch(
                "CREATE TEMP TABLE updates (n INTEGER NOT NULL);
                 INSERT INTO updates (n) VALUES (0);
                 CREATE TEMP TRIGGER count_track_updates AFTER UPDATE ON main.tracks BEGIN
                     UPDATE updates SET n = n + 1;
                 END;
                 CREATE TEMP TRIGGER count_album_updates AFTER UPDATE ON main.albums BEGIN
                     UPDATE updates SET n = n + 1;
                 END;",
            )
            .unwrap();
        }
        let before = env.search3_requests();
        // Over a day later: a full pass although lastModified did not change.
        let fired = Cell::new(0);
        let report = sync_library(&env.db, &env.client, NOW + DAY_MS + 1, &|_| {}, &|| {
            fired.set(fired.get() + 1)
        })
        .unwrap();
        assert!(report.full_pass);
        assert!(!report.changed);
        assert!(env.search3_requests() > before);
        assert_eq!(env.count("SELECT n FROM temp.updates"), 0);
        assert_eq!(fired.get(), 0);

        // One changed song is one rewrite.
        env.server
            .edit(|lib| lib.songs[3].genre = Some("Demo Genre".into()));
        let report = env.sync_at(NOW + DAY_MS + 2).unwrap();
        assert!(report.changed);
        assert_eq!(env.count("SELECT n FROM temp.updates"), 1);
    }

    #[test]
    fn album_artist_id_is_always_written() {
        let mut lib = FakeLibrary::synthetic(2, 2);
        lib.albums[0].artist_id = None;
        let env = env(lib);
        env.sync().unwrap();
        let artist_id = |id: &str| -> Option<String> {
            let conn = env.db.0.lock().unwrap();
            conn.query_row("SELECT artist_id FROM albums WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .unwrap()
        };
        assert_eq!(artist_id("navidrome:al-0"), None);
        assert_eq!(
            artist_id("navidrome:al-1").as_deref(),
            Some("navidrome:ar-0")
        );
        // The server names the artist later: the existing row gets it.
        env.server
            .edit(|lib| lib.albums[0].artist_id = Some("ar-7".into()));
        env.sync().unwrap();
        assert_eq!(
            artist_id("navidrome:al-0").as_deref(),
            Some("navidrome:ar-7")
        );
        // And through the getAlbum fallback.
        env.server
            .edit(|lib| lib.albums[1].artist_id = Some("ar-8".into()));
        env.server.set_fault(Fault::Search3Empty);
        env.sync().unwrap();
        assert_eq!(
            artist_id("navidrome:al-1").as_deref(),
            Some("navidrome:ar-8")
        );
    }

    fn fixture_songs() -> Vec<Child> {
        let path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "tests",
            "fixtures",
            "subsonic",
            "search3.json",
        ]
        .iter()
        .collect();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        serde_json::from_value(value["subsonic-response"]["searchResult3"]["song"].clone()).unwrap()
    }

    #[test]
    fn song_row_maps_fields() {
        let songs = fixture_songs();
        let track = song_row(&songs[0], None);
        assert_eq!(
            track,
            Track {
                id: "navidrome:tr-1".into(),
                source: Source::Navidrome,
                title: "Demo Song 1".into(),
                artist: "Demo Artist 1".into(),
                album_artist: "Demo Artist 1".into(),
                album: "Demo Album 1".into(),
                album_id: "navidrome:al-1".into(),
                track_no: Some(1),
                disc_no: Some(1),
                duration_ms: 222_000,
                year: Some(2004),
                genre: Some("Rock".into()),
                format: "flac".into(),
                sample_rate: Some(44_100),
                bit_depth: Some(16),
                bitrate_kbps: Some(903),
                cover_id: Some("nd-al-1_abc".into()),
                path: None,
                replay_gain_track_db: Some(-6.5),
                replay_gain_album_db: Some(-7.25),
                replay_gain_track_peak: Some(0.98),
                replay_gain_album_peak: Some(1.0),
            }
        );

        // displayAlbumArtist wins over the song artist; bitDepth 0 and an empty replayGain
        // are unknown.
        let track = song_row(&songs[1], None);
        assert_eq!(track.artist, "Demo Artist 2 feat. Demo Artist 1");
        assert_eq!(track.album_artist, "Demo Artist 2");
        assert_eq!(track.bit_depth, None);
        assert_eq!(track.disc_no, None);
        assert_eq!(track.replay_gain_track_db, None);
        assert_eq!(track.format, "mp3");

        // No album, no album artist, no cover.
        let track = song_row(&songs[2], None);
        assert_eq!(track.album_artist, "Sample Band");
        assert_eq!(track.album, "");
        assert_eq!(track.album_id, "");
        assert_eq!(track.cover_id, None);
        assert_eq!(track.format, "ogg");

        // albumArtists without displayAlbumArtist; the format from the content type.
        let mut song = songs[1].clone();
        song.display_album_artist = None;
        song.suffix = None;
        song.content_type = Some("audio/x-FLAC".into());
        song.replay_gain = Some(ReplayGainInfo {
            album_gain: Some(-3.0),
            ..ReplayGainInfo::default()
        });
        let track = song_row(&song, None);
        assert_eq!(track.album_artist, "Demo Artist 2");
        assert_eq!(track.format, "x-flac");
        assert_eq!(track.replay_gain_album_db, Some(-3.0));
        assert_eq!(track.replay_gain_track_db, None);
    }

    #[test]
    fn album_row_maps_fields() {
        let album = AlbumId3 {
            id: "al-1".into(),
            name: "Demo Album 1".into(),
            artist: Some("Demo Artist 1".into()),
            artist_id: Some("ar-1".into()),
            cover_art: Some("al-1_abc".into()),
            song_count: 9,
            duration: 2_000,
            year: Some(2004),
            ..AlbumId3::default()
        };
        let (row, artist_id) = album_row(&album);
        assert_eq!(
            row,
            Album {
                id: "navidrome:al-1".into(),
                source: Source::Navidrome,
                title: "Demo Album 1".into(),
                artist: "Demo Artist 1".into(),
                year: Some(2004),
                cover_id: Some("nd-al-1_abc".into()),
                track_count: 9,
                duration_ms: 2_000_000,
            }
        );
        assert_eq!(artist_id.as_deref(), Some("navidrome:ar-1"));
        let (row, artist_id) = album_row(&AlbumId3 {
            id: "al-2".into(),
            name: "Loose".into(),
            artist_id: Some(String::new()),
            cover_art: Some("../x".into()),
            year: Some(0),
            ..AlbumId3::default()
        });
        assert_eq!(artist_id, None);
        assert_eq!(row.cover_id, None);
        assert_eq!(row.year, None);
    }

    #[test]
    fn tracks_use_the_album_cover() {
        let songs = fixture_songs();
        assert_eq!(
            song_row(&songs[0], Some("al-9_album")).cover_id.as_deref(),
            Some("nd-al-9_album")
        );
        assert_eq!(
            song_row(&songs[0], None).cover_id.as_deref(),
            Some("nd-al-1_abc")
        );

        let mut lib = FakeLibrary::synthetic(2, 3);
        for song in &mut lib.songs {
            song.cover_art = Some(format!("mf-{}", song.id));
        }
        // A song whose album the album list does not know keeps its own cover.
        lib.songs[5].album_id = Some("al-unlisted".into());
        let env = env(lib);
        env.sync().unwrap();
        assert_eq!(
            env.count(
                "SELECT COUNT(*) FROM tracks WHERE album_id = 'navidrome:al-0'
                 AND cover_id = 'nd-al-0_cover'"
            ),
            3
        );
        let conn = env.db.0.lock().unwrap();
        let cover: Option<String> = conn
            .query_row(
                "SELECT cover_id FROM tracks WHERE id = 'navidrome:tr-5'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cover.as_deref(), Some("nd-mf-tr-5"));
    }

    #[test]
    fn short_song_listing_is_refused() {
        let env = env(FakeLibrary::synthetic(4, 10));
        env.sync().unwrap();
        let stored = env.state("last_modified");
        // The albums still claim 40 songs, the song listing has 12 of them.
        env.server.edit(|lib| lib.songs.truncate(12));
        let result = env.sync();
        match result {
            Err(SyncError::Refused(reason)) => assert!(reason.contains("12 of 40"), "{reason}"),
            other => panic!("expected a refused pass, got {other:?}"),
        }
        assert_eq!(env.nd_tracks(), 40);
        assert_eq!(env.state("last_modified"), stored);

        // Half of the claimed songs is still accepted (song counts can lag a little).
        env.server.edit(|lib| {
            lib.songs = FakeLibrary::synthetic(4, 10).songs;
            lib.songs.truncate(20);
        });
        env.sync().unwrap();
        assert_eq!(env.nd_tracks(), 20);
    }

    /// A first sync of 30k songs in 3000 albums into a database file, as on a real machine,
    /// then a full pass after the server's `lastModified` moved with nothing else changed.
    /// Timing is only meaningful in an optimized build: `cargo test --release
    /// --manifest-path src-tauri/Cargo.toml sync_30k_synthetic -- --ignored --nocapture`.
    #[test]
    #[ignore = "30k songs over HTTP; run with --release"]
    fn sync_30k_synthetic() {
        let server = FakeServer::start(FakeLibrary::synthetic(3000, 10));
        let client = SubsonicClient::new(server.account()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("library.db")).unwrap();

        let started = Instant::now();
        let report = sync_library(&db, &client, NOW, &|_| {}, &|| {}).unwrap();
        let first = started.elapsed();
        assert_eq!(
            report,
            SyncReport {
                full_pass: true,
                tracks: 30_000,
                albums: 3_000,
                changed: true,
            }
        );

        server.edit(|lib| lib.last_modified += 1);
        let started = Instant::now();
        let report = sync_library(&db, &client, NOW + 60_000, &|_| {}, &|| {}).unwrap();
        let again = started.elapsed();
        assert!(report.full_pass);
        assert!(!report.changed, "{report:?}");
        assert_eq!(report.tracks, 30_000);

        println!("sync of 30000 songs: first {first:?}, unchanged full pass {again:?}");
        let conn = db.0.lock().unwrap();
        let found = query::search(&conn, "Demo Song 29999", 10).unwrap();
        assert!(found.tracks.iter().any(|t| t.id == "navidrome:tr-29998"));
        assert!(first.as_secs() < 60, "first sync took {first:?}");
        assert!(again.as_secs() < 60, "unchanged full pass took {again:?}");
    }

    #[test]
    fn cancelled_pass_writes_nothing_more() {
        let env = env(FakeLibrary::synthetic(100, 12));
        let cancelled = std::cell::Cell::new(false);
        let result = sync_library_cancellable(
            &env.db,
            &env.client,
            NOW,
            &|p| {
                if p.done == 500 {
                    cancelled.set(true);
                }
            },
            &|| {},
            &|| cancelled.get(),
        );
        assert!(matches!(result, Err(SyncError::Cancelled)), "{result:?}");
        assert_eq!(env.nd_tracks(), 500);
        assert_eq!(env.state("last_modified"), None);
        assert_eq!(env.state("last_sync"), None);

        // A pass with nothing to do stores no sync time once cancelled either.
        env.sync().unwrap();
        let last_sync = env.state("last_sync");
        let result =
            sync_library_cancellable(&env.db, &env.client, NOW + 1, &|_| {}, &|| {}, &|| true);
        assert!(matches!(result, Err(SyncError::Cancelled)), "{result:?}");
        assert_eq!(env.state("last_sync"), last_sync);
    }
}
