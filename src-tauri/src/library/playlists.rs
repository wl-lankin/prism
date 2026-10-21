//! User playlists: named, ordered lists of track ids (duplicates allowed).
//!
//! An entry is a plain `(playlist_id, position, track_id)` row with no foreign key to `tracks`:
//! one whose track is not in the index (folder offline, file removed, other source not loaded)
//! is kept with `track: None` and resolves again when the track returns. Positions are always
//! 0..n; every change that could leave a gap renumbers.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::bail;
use rusqlite::{params, Connection, OptionalExtension, ToSql, Transaction};
use serde::Serialize;

use super::model::Track;
use super::query::{collect, track_from_row, TRACK_COLUMNS};

/// Emitted (no payload) after every effective change.
pub const EVENT_CHANGED: &str = "playlists://changed";

/// Longest accepted name, in characters (after trimming).
pub const MAX_NAME_CHARS: usize = 200;

/// How many distinct covers a playlist offers for its mosaic.
pub(crate) const COVER_MOSAIC: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    /// Entries, including those whose track is not in the index.
    pub track_count: u32,
    /// Summed over the entries whose track is in the index.
    pub duration_ms: u64,
    /// Up to four distinct cover ids, in playlist order.
    pub cover_ids: Vec<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistEntry {
    pub position: u32,
    pub track_id: String,
    /// `None` when the track is not in the index.
    pub track: Option<Track>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn clean_name(name: &str) -> anyhow::Result<String> {
    let name = name.trim();
    let chars = name.chars().count();
    if chars == 0 {
        bail!("A playlist needs a name");
    }
    if chars > MAX_NAME_CHARS {
        bail!("A playlist name can have at most {MAX_NAME_CHARS} characters");
    }
    Ok(name.to_string())
}

fn ensure_exists(conn: &Connection, id: i64) -> anyhow::Result<()> {
    let found: Option<i64> = conn
        .query_row("SELECT 1 FROM playlists WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()?;
    if found.is_none() {
        bail!("This playlist no longer exists");
    }
    Ok(())
}

fn touch(conn: &Connection, id: i64) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE playlists SET updated_at = ?2 WHERE id = ?1",
        params![id, now_ms()],
    )?;
    Ok(())
}

fn track_ids(conn: &Connection, id: i64) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn
        .prepare("SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position")?;
    let rows = stmt.query_map([id], |row| row.get(0))?;
    Ok(rows.collect::<Result<Vec<String>, _>>()?)
}

/// Replaces the entries of a playlist with `ids`, numbered 0..n.
fn write_entries(tx: &Transaction, id: i64, ids: &[String]) -> anyhow::Result<()> {
    tx.execute("DELETE FROM playlist_tracks WHERE playlist_id = ?1", [id])?;
    let mut insert = tx.prepare(
        "INSERT INTO playlist_tracks (playlist_id, position, track_id) VALUES (?1, ?2, ?3)",
    )?;
    for (position, track_id) in ids.iter().enumerate() {
        insert.execute(params![id, position as i64, track_id])?;
    }
    Ok(())
}

fn summarize(conn: &Connection, id: i64) -> anyhow::Result<Playlist> {
    let (name, updated_at): (String, i64) = conn.query_row(
        "SELECT name, updated_at FROM playlists WHERE id = ?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let track_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM playlist_tracks WHERE playlist_id = ?1",
        [id],
        |row| row.get(0),
    )?;
    let duration_ms: i64 = conn.query_row(
        "SELECT COALESCE(SUM(t.duration_ms), 0) FROM playlist_tracks p
         JOIN tracks t ON t.id = p.track_id WHERE p.playlist_id = ?1",
        [id],
        |row| row.get(0),
    )?;
    let mut stmt = conn.prepare(
        "SELECT t.cover_id FROM playlist_tracks p JOIN tracks t ON t.id = p.track_id
         WHERE p.playlist_id = ?1 AND t.cover_id IS NOT NULL ORDER BY p.position",
    )?;
    let mut cover_ids: Vec<String> = Vec::with_capacity(COVER_MOSAIC);
    let rows = stmt.query_map([id], |row| row.get::<_, String>(0))?;
    for cover in rows {
        let cover = cover?;
        if !cover_ids.contains(&cover) {
            cover_ids.push(cover);
            if cover_ids.len() == COVER_MOSAIC {
                break;
            }
        }
    }
    Ok(Playlist {
        id,
        name,
        track_count: track_count as u32,
        duration_ms: duration_ms.max(0) as u64,
        cover_ids,
        updated_at,
    })
}

/// A CTE `covers(playlist_id, cover_id, rank)` over the entry table `entries`: each playlist's
/// distinct cover ids, ranked 1.. by the first position they appear at. Entries whose track is
/// not in the index have no cover and drop out. Join it with `rank <= COVER_MOSAIC`.
pub(crate) fn covers_cte(entries: &str) -> String {
    format!(
        "covers AS (
            SELECT p.playlist_id, t.cover_id,
                   ROW_NUMBER() OVER (PARTITION BY p.playlist_id ORDER BY MIN(p.position)) AS rank
            FROM {entries} p JOIN tracks t ON t.id = p.track_id
            WHERE t.cover_id IS NOT NULL
            GROUP BY p.playlist_id, t.cover_id
        )"
    )
}

/// Folds the rows of a summary query, one row per `(playlist, cover)` ordered by playlist and
/// then cover rank (a playlist without covers has one row with `None`), into one summary per
/// playlist in row order.
pub(crate) fn fold_covers<K: PartialEq, T>(
    rows: impl IntoIterator<Item = (K, T, Option<String>)>,
    covers: impl Fn(&mut T) -> &mut Vec<String>,
) -> Vec<T> {
    let mut out: Vec<(K, T)> = Vec::new();
    for (key, summary, cover) in rows {
        if out.last().is_none_or(|(last, _)| *last != key) {
            out.push((key, summary));
        }
        if let (Some(cover), Some((_, summary))) = (cover, out.last_mut()) {
            covers(summary).push(cover);
        }
    }
    out.into_iter().map(|(_, summary)| summary).collect()
}

/// Every playlist, most recently changed first, in one query.
pub fn playlists_list(conn: &Connection) -> anyhow::Result<Vec<Playlist>> {
    let sql = format!(
        "WITH totals AS (
            SELECT p.playlist_id, COUNT(*) AS track_count,
                   COALESCE(SUM(t.duration_ms), 0) AS duration_ms
            FROM playlist_tracks p LEFT JOIN tracks t ON t.id = p.track_id
            GROUP BY p.playlist_id
        ),
        {covers}
        SELECT pl.id, pl.name, pl.updated_at, COALESCE(n.track_count, 0),
               COALESCE(n.duration_ms, 0), c.cover_id
        FROM playlists pl
        LEFT JOIN totals n ON n.playlist_id = pl.id
        LEFT JOIN covers c ON c.playlist_id = pl.id AND c.rank <= {COVER_MOSAIC}
        ORDER BY pl.updated_at DESC, pl.id DESC, c.rank",
        covers = covers_cte("playlist_tracks"),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        let id: i64 = row.get(0)?;
        let track_count: i64 = row.get(3)?;
        let duration_ms: i64 = row.get(4)?;
        let playlist = Playlist {
            id,
            name: row.get(1)?,
            track_count: track_count.max(0) as u32,
            duration_ms: duration_ms.max(0) as u64,
            cover_ids: Vec::new(),
            updated_at: row.get(2)?,
        };
        Ok((id, playlist, row.get::<_, Option<String>>(5)?))
    })?;
    let rows = rows.collect::<Result<Vec<_>, _>>()?;
    Ok(fold_covers(rows, |playlist| &mut playlist.cover_ids))
}

pub fn playlist_create(conn: &Connection, name: &str) -> anyhow::Result<Playlist> {
    let name = clean_name(name)?;
    let now = now_ms();
    conn.execute(
        "INSERT INTO playlists (name, created_at, updated_at) VALUES (?1, ?2, ?2)",
        params![name, now],
    )?;
    summarize(conn, conn.last_insert_rowid())
}

/// Renames the playlist; returns whether the name changed (the same name changes nothing).
pub fn playlist_rename(conn: &Connection, id: i64, name: &str) -> anyhow::Result<bool> {
    let name = clean_name(name)?;
    ensure_exists(conn, id)?;
    let rows = conn.execute(
        "UPDATE playlists SET name = ?2, updated_at = ?3 WHERE id = ?1 AND name IS NOT ?2",
        params![id, name, now_ms()],
    )?;
    Ok(rows > 0)
}

/// Deletes the playlist and, through the foreign key, its entries. Idempotent; returns whether
/// a playlist was deleted.
pub fn playlist_delete(conn: &Connection, id: i64) -> anyhow::Result<bool> {
    Ok(conn.execute("DELETE FROM playlists WHERE id = ?1", [id])? > 0)
}

/// The entries of `playlist_id` in the entry table `entries` (columns `playlist_id`,
/// `position`, `track_id`), in order and numbered 0..n; a track that is not in the index comes
/// back with `track: None`.
pub(crate) fn resolve_entries(
    conn: &Connection,
    entries: &str,
    playlist_id: &dyn ToSql,
) -> anyhow::Result<Vec<PlaylistEntry>> {
    let sql = format!(
        "SELECT {TRACK_COLUMNS} FROM tracks t
         WHERE t.id IN (SELECT track_id FROM {entries} WHERE playlist_id = ?1)"
    );
    let resolved: HashMap<String, Track> = collect(conn, &sql, [playlist_id], track_from_row)?
        .into_iter()
        .map(|track| (track.id.clone(), track))
        .collect();
    let mut stmt = conn.prepare(&format!(
        "SELECT track_id FROM {entries} WHERE playlist_id = ?1 ORDER BY position"
    ))?;
    let ids = stmt
        .query_map([playlist_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(ids
        .into_iter()
        .enumerate()
        .map(|(position, track_id)| PlaylistEntry {
            position: position as u32,
            track: resolved.get(&track_id).cloned(),
            track_id,
        })
        .collect())
}

/// The entries in order; a track that is not in the index comes back with `track: None`.
pub fn playlist_entries(conn: &Connection, id: i64) -> anyhow::Result<Vec<PlaylistEntry>> {
    ensure_exists(conn, id)?;
    resolve_entries(conn, "playlist_tracks", &id)
}

/// Appends the tracks in the given order; the same track may be added again. Returns whether
/// anything was added.
pub fn playlist_add(conn: &Connection, id: i64, track_ids: &[String]) -> anyhow::Result<bool> {
    ensure_exists(conn, id)?;
    if track_ids.is_empty() {
        return Ok(false);
    }
    if track_ids.iter().any(|track_id| track_id.is_empty()) {
        bail!("A track id cannot be empty");
    }
    let tx = conn.unchecked_transaction()?;
    let next: i64 = tx.query_row(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM playlist_tracks WHERE playlist_id = ?1",
        [id],
        |row| row.get(0),
    )?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO playlist_tracks (playlist_id, position, track_id) VALUES (?1, ?2, ?3)",
        )?;
        for (offset, track_id) in track_ids.iter().enumerate() {
            insert.execute(params![id, next + offset as i64, track_id])?;
        }
    }
    touch(&tx, id)?;
    tx.commit()?;
    Ok(true)
}

/// Removes the entries at `positions` (unknown positions are ignored) and renumbers the rest.
/// Returns whether anything was removed.
pub fn playlist_remove(conn: &Connection, id: i64, positions: &[u32]) -> anyhow::Result<bool> {
    ensure_exists(conn, id)?;
    let tx = conn.unchecked_transaction()?;
    let before = track_ids(&tx, id)?;
    let kept: Vec<String> = before
        .iter()
        .enumerate()
        .filter(|(position, _)| !positions.contains(&(*position as u32)))
        .map(|(_, track_id)| track_id.clone())
        .collect();
    let changed = kept.len() != before.len();
    if changed {
        write_entries(&tx, id, &kept)?;
        touch(&tx, id)?;
    }
    tx.commit()?;
    Ok(changed)
}

/// Moves the entry at `from` so it ends up at index `to` (both within 0..n). Returns whether
/// the order changed (not for `from == to`, nor for a move between equal tracks).
pub fn playlist_move(conn: &Connection, id: i64, from: u32, to: u32) -> anyhow::Result<bool> {
    ensure_exists(conn, id)?;
    let tx = conn.unchecked_transaction()?;
    let before = track_ids(&tx, id)?;
    let (from, to) = (from as usize, to as usize);
    if from >= before.len() || to >= before.len() {
        bail!("That position is outside the playlist");
    }
    let mut ids = before.clone();
    let moved = ids.remove(from);
    ids.insert(to, moved);
    let changed = ids != before;
    if changed {
        write_entries(&tx, id, &ids)?;
        touch(&tx, id)?;
    }
    tx.commit()?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::rekey::rekey_tracks;
    use crate::db::Db;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn insert_track(conn: &Connection, id: &str, duration_ms: i64, cover: Option<&str>) {
        conn.execute(
            "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id,
                duration_ms, format, cover_id)
             VALUES (?1, 'local', ?1, 'A', 'A', 'B', 'local:album', ?2, 'flac', ?3)",
            params![id, duration_ms, cover],
        )
        .unwrap();
    }

    fn ids_of(conn: &Connection, id: i64) -> Vec<(u32, String)> {
        playlist_entries(conn, id)
            .unwrap()
            .into_iter()
            .map(|entry| (entry.position, entry.track_id))
            .collect()
    }

    fn order(conn: &Connection, id: i64) -> Vec<String> {
        ids_of(conn, id)
            .into_iter()
            .map(|(_, track)| track)
            .collect()
    }

    fn strings(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    fn filled(conn: &Connection, tracks: &[&str]) -> i64 {
        let playlist = playlist_create(conn, "Mix").unwrap();
        playlist_add(conn, playlist.id, &strings(tracks)).unwrap();
        playlist.id
    }

    #[test]
    fn create_add_and_list() {
        let db = db();
        let conn = db.0.lock().unwrap();
        insert_track(&conn, "a", 1000, Some("c1"));
        insert_track(&conn, "b", 2000, Some("c1"));
        insert_track(&conn, "c", 4000, Some("c2"));
        insert_track(&conn, "d", 8000, None);
        insert_track(&conn, "e", 16000, Some("c3"));
        insert_track(&conn, "f", 32000, Some("c4"));
        insert_track(&conn, "g", 64000, Some("c5"));

        let created = playlist_create(&conn, "  Road trip  ").unwrap();
        assert_eq!(created.name, "Road trip");
        assert_eq!(created.track_count, 0);
        assert_eq!(created.duration_ms, 0);
        assert!(created.cover_ids.is_empty());

        playlist_add(
            &conn,
            created.id,
            &strings(&["a", "b", "c", "d", "e", "f", "g"]),
        )
        .unwrap();
        playlist_add(&conn, created.id, &strings(&["a", "local:gone"])).unwrap();
        let listed = playlist_list_one(&conn, created.id);
        assert_eq!(listed.track_count, 9);
        // "local:gone" is not in the index and adds nothing to the duration.
        assert_eq!(
            listed.duration_ms,
            1000 + 2000 + 4000 + 8000 + 16000 + 32000 + 64000 + 1000
        );
        // Distinct, in playlist order, at most four, none for a track without a cover.
        assert_eq!(listed.cover_ids, strings(&["c1", "c2", "c3", "c4"]));
        // Duplicates are kept as separate entries.
        assert_eq!(order(&conn, created.id)[0], "a");
        assert_eq!(order(&conn, created.id)[7], "a");
        assert_eq!(
            ids_of(&conn, created.id)
                .iter()
                .map(|(p, _)| *p)
                .collect::<Vec<_>>(),
            (0..9).collect::<Vec<u32>>()
        );
    }

    fn playlist_list_one(conn: &Connection, id: i64) -> Playlist {
        playlists_list(conn)
            .unwrap()
            .into_iter()
            .find(|playlist| playlist.id == id)
            .unwrap()
    }

    #[test]
    fn list_is_newest_change_first() {
        let db = db();
        let conn = db.0.lock().unwrap();
        let first = playlist_create(&conn, "One").unwrap();
        let second = playlist_create(&conn, "Two").unwrap();
        conn.execute("UPDATE playlists SET updated_at = id * 10", [])
            .unwrap();
        let names: Vec<String> = playlists_list(&conn)
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, strings(&["Two", "One"]));
        // Renaming and adding both count as a change.
        conn.execute("UPDATE playlists SET updated_at = 0", [])
            .unwrap();
        playlist_rename(&conn, first.id, "Uno").unwrap();
        let names: Vec<String> = playlists_list(&conn)
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, strings(&["Uno", "Two"]));
        conn.execute("UPDATE playlists SET updated_at = 0", [])
            .unwrap();
        playlist_add(&conn, second.id, &strings(&["x"])).unwrap();
        assert_eq!(playlists_list(&conn).unwrap()[0].id, second.id);
    }

    #[test]
    fn remove_renumbers() {
        let db = db();
        let conn = db.0.lock().unwrap();
        let id = filled(&conn, &["a", "b", "c", "d", "e"]);
        playlist_remove(&conn, id, &[1, 3, 3, 99]).unwrap();
        assert_eq!(
            ids_of(&conn, id),
            vec![
                (0, "a".to_string()),
                (1, "c".to_string()),
                (2, "e".to_string())
            ]
        );
        playlist_remove(&conn, id, &[0, 1, 2]).unwrap();
        assert!(ids_of(&conn, id).is_empty());
        // Appending after a removal starts at 0 again.
        playlist_add(&conn, id, &strings(&["z"])).unwrap();
        assert_eq!(ids_of(&conn, id), vec![(0, "z".to_string())]);
    }

    #[test]
    fn move_entry_up_and_down() {
        let db = db();
        let conn = db.0.lock().unwrap();
        let id = filled(&conn, &["a", "b", "c", "d"]);
        playlist_move(&conn, id, 3, 1).unwrap();
        assert_eq!(order(&conn, id), strings(&["a", "d", "b", "c"]));
        playlist_move(&conn, id, 0, 2).unwrap();
        assert_eq!(order(&conn, id), strings(&["d", "b", "a", "c"]));
        playlist_move(&conn, id, 2, 2).unwrap();
        assert_eq!(order(&conn, id), strings(&["d", "b", "a", "c"]));
        assert!(playlist_move(&conn, id, 4, 0).is_err());
        assert!(playlist_move(&conn, id, 0, 4).is_err());
        assert_eq!(order(&conn, id), strings(&["d", "b", "a", "c"]));
        assert_eq!(
            ids_of(&conn, id)
                .iter()
                .map(|(p, _)| *p)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
    }

    #[test]
    fn foreign_keys_are_on_for_the_connection() {
        let db = db();
        let conn = db.0.lock().unwrap();
        let on: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(on, 1);
        // An entry for a playlist that does not exist is refused by the schema itself.
        assert!(conn
            .execute(
                "INSERT INTO playlist_tracks (playlist_id, position, track_id) VALUES (999, 0, 'x')",
                [],
            )
            .is_err());
    }

    #[test]
    fn delete_cascades_entries() {
        let db = db();
        let conn = db.0.lock().unwrap();
        let doomed = filled(&conn, &["a", "b"]);
        let kept = filled(&conn, &["c"]);
        assert!(playlist_delete(&conn, doomed).unwrap());
        assert!(!playlist_delete(&conn, doomed).unwrap());
        let rows = |id: i64| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM playlist_tracks WHERE playlist_id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(rows(doomed), 0);
        assert_eq!(rows(kept), 1);
        assert_eq!(playlists_list(&conn).unwrap().len(), 1);
        assert!(playlist_entries(&conn, doomed).is_err());
    }

    #[test]
    fn blank_name_is_refused() {
        let db = db();
        let conn = db.0.lock().unwrap();
        assert!(playlist_create(&conn, "").is_err());
        assert!(playlist_create(&conn, "  \t\n ").is_err());
        assert!(playlist_create(&conn, &"x".repeat(MAX_NAME_CHARS + 1)).is_err());
        // The limit counts characters, not bytes, and applies after trimming.
        let longest = format!("  {}  ", "\u{e4}".repeat(MAX_NAME_CHARS));
        let created = playlist_create(&conn, &longest).unwrap();
        assert_eq!(created.name.chars().count(), MAX_NAME_CHARS);
        assert!(playlist_rename(&conn, created.id, " ").is_err());
        assert_eq!(
            playlists_list(&conn).unwrap()[0].name.chars().count(),
            MAX_NAME_CHARS
        );
        // Duplicate names are fine.
        playlist_create(&conn, "Same").unwrap();
        playlist_create(&conn, "Same").unwrap();
        assert_eq!(playlists_list(&conn).unwrap().len(), 3);
        playlist_rename(&conn, created.id, "  Renamed ").unwrap();
        assert_eq!(playlist_list_one(&conn, created.id).name, "Renamed");
    }

    #[test]
    fn missing_playlist_is_an_error() {
        let db = db();
        let conn = db.0.lock().unwrap();
        assert!(playlist_rename(&conn, 42, "x").is_err());
        assert!(playlist_add(&conn, 42, &strings(&["a"])).is_err());
        assert!(playlist_remove(&conn, 42, &[0]).is_err());
        assert!(playlist_move(&conn, 42, 0, 1).is_err());
    }

    #[test]
    fn missing_track_entry_is_kept() {
        let db = db();
        let conn = db.0.lock().unwrap();
        insert_track(&conn, "here", 1000, None);
        let id = filled(&conn, &["here", "navidrome:abc", "here"]);
        let entries = playlist_entries(&conn, id).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries[0].track.as_ref().map(|t| t.id.as_str()),
            Some("here")
        );
        assert_eq!(entries[1].track_id, "navidrome:abc");
        assert!(entries[1].track.is_none());
        assert!(entries[2].track.is_some());
        // The track returns: the entry resolves again without any edit.
        insert_track(&conn, "navidrome:abc", 500, None);
        assert!(playlist_entries(&conn, id).unwrap()[1].track.is_some());
    }

    #[test]
    fn noop_rename_reports_no_change() {
        let db = db();
        let conn = db.0.lock().unwrap();
        let id = filled(&conn, &["a", "b"]);
        conn.execute("UPDATE playlists SET updated_at = 7", [])
            .unwrap();
        // The same name, also after trimming: nothing changes, not even the change time.
        assert!(!playlist_rename(&conn, id, "Mix").unwrap());
        assert!(!playlist_rename(&conn, id, "  Mix ").unwrap());
        assert_eq!(playlist_list_one(&conn, id).updated_at, 7);
        assert!(playlist_rename(&conn, id, "mix").unwrap());
        assert_eq!(playlist_list_one(&conn, id).name, "mix");
        assert_ne!(playlist_list_one(&conn, id).updated_at, 7);
    }

    #[test]
    fn noop_edits_report_no_change() {
        let db = db();
        let conn = db.0.lock().unwrap();
        let id = filled(&conn, &["a", "a", "b"]);
        conn.execute("UPDATE playlists SET updated_at = 7", [])
            .unwrap();
        assert!(!playlist_add(&conn, id, &[]).unwrap());
        assert!(!playlist_remove(&conn, id, &[]).unwrap());
        assert!(!playlist_remove(&conn, id, &[3, 99]).unwrap());
        assert!(!playlist_move(&conn, id, 1, 1).unwrap());
        // Two equal tracks swap places: the order reads the same.
        assert!(!playlist_move(&conn, id, 0, 1).unwrap());
        assert_eq!(playlist_list_one(&conn, id).updated_at, 7);
        assert!(playlist_move(&conn, id, 2, 0).unwrap());
        assert_eq!(order(&conn, id), strings(&["b", "a", "a"]));
        assert!(playlist_remove(&conn, id, &[0]).unwrap());
        assert!(playlist_add(&conn, id, &strings(&["c"])).unwrap());
        assert_eq!(order(&conn, id), strings(&["a", "a", "c"]));
    }

    #[test]
    fn playlists_list_counts_and_covers_unchanged() {
        let db = db();
        let conn = db.0.lock().unwrap();
        insert_track(&conn, "a", 1000, Some("c1"));
        insert_track(&conn, "b", 2000, Some("c2"));
        insert_track(&conn, "c", 4000, None);
        insert_track(&conn, "d", 8000, Some("c3"));
        insert_track(&conn, "e", 16000, Some("c4"));
        insert_track(&conn, "f", 32000, Some("c5"));
        insert_track(&conn, "g", 64000, Some("c1"));
        let empty = playlist_create(&conn, "Empty").unwrap().id;
        let gone_only = filled(&conn, &["navidrome:x", "local:gone"]);
        let mixed = filled(&conn, &["c", "navidrome:x", "b", "a", "b", "g"]);
        let many = filled(&conn, &["f", "e", "a", "g", "d", "b", "c"]);
        let one = filled(&conn, &["d"]);
        conn.execute("UPDATE playlists SET updated_at = 100 - id", [])
            .unwrap();
        conn.execute(
            "UPDATE playlists SET updated_at = 1000 WHERE id = ?1",
            [many],
        )
        .unwrap();

        let listed = playlists_list(&conn).unwrap();
        // The one query gives exactly what the per-playlist summary gives, in the same order.
        let expected: Vec<Playlist> = [many, empty, gone_only, mixed, one]
            .into_iter()
            .map(|id| summarize(&conn, id).unwrap())
            .collect();
        assert_eq!(listed, expected);

        let by_id = |id: i64| listed.iter().find(|p| p.id == id).unwrap().clone();
        assert_eq!(by_id(empty).track_count, 0);
        assert_eq!(by_id(empty).duration_ms, 0);
        assert!(by_id(empty).cover_ids.is_empty());
        // Entries whose track is missing count, add no duration and no cover.
        assert_eq!(by_id(gone_only).track_count, 2);
        assert_eq!(by_id(gone_only).duration_ms, 0);
        assert!(by_id(gone_only).cover_ids.is_empty());
        assert_eq!(by_id(mixed).track_count, 6);
        assert_eq!(by_id(mixed).duration_ms, 4000 + 2000 + 1000 + 2000 + 64000);
        assert_eq!(by_id(mixed).cover_ids, strings(&["c2", "c1"]));
        assert_eq!(by_id(many).cover_ids, strings(&["c5", "c4", "c1", "c3"]));
        assert_eq!(by_id(one).cover_ids, strings(&["c3"]));
    }

    #[test]
    fn rekey_updates_playlist_entries() {
        let db = db();
        let mut conn = db.0.lock().unwrap();
        insert_track(&conn, "old-1", 1000, None);
        insert_track(&conn, "old-2", 1000, None);
        let id = filled(&conn, &["old-1", "old-2", "old-1"]);
        let map = vec![
            ("old-1".to_string(), "new-1".to_string()),
            ("old-2".to_string(), "new-2".to_string()),
        ];
        let tx = conn.transaction().unwrap();
        rekey_tracks(&tx, &map).unwrap();
        tx.commit().unwrap();
        // Entries keep their positions, and a repeated track stays repeated.
        assert_eq!(
            ids_of(&conn, id),
            vec![
                (0, "new-1".to_string()),
                (1, "new-2".to_string()),
                (2, "new-1".to_string())
            ]
        );
        assert!(playlist_entries(&conn, id)
            .unwrap()
            .iter()
            .all(|e| e.track.is_some()));
    }
}
