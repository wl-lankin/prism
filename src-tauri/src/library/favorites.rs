//! Local favorites: tracks (by track id), albums (by album id) and artists (by name).
//!
//! A favorite is a plain row with no foreign key: one whose item is not in the index (folder
//! offline, file removed) is kept, not listed, and listed again when the item returns.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::model::{Album, Artist, Track};
use super::query::{
    album_from_row, artist_from_row, artist_select, collect, track_from_row, ALBUM_COLUMNS,
    TRACK_COLUMNS,
};

/// Emitted (no payload) after every effective change.
pub const EVENT_CHANGED: &str = "favorites://changed";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FavKind {
    Track,
    Album,
    Artist,
}

impl FavKind {
    fn as_str(self) -> &'static str {
        match self {
            FavKind::Track => "track",
            FavKind::Album => "album",
            FavKind::Artist => "artist",
        }
    }
}

/// Every favorite id, newest first, whether or not the item is in the index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteIds {
    pub tracks: Vec<String>,
    pub albums: Vec<String>,
    pub artists: Vec<String>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Sets the favorite state and returns whether anything changed (what the event hangs on).
pub fn set_changed(conn: &Connection, kind: FavKind, id: &str, on: bool) -> anyhow::Result<bool> {
    let changed = if on {
        conn.execute(
            "INSERT INTO favorites (kind, item_id, source, added_at) VALUES (?1, ?2, 'local', ?3)
             ON CONFLICT(kind, item_id) DO NOTHING",
            params![kind.as_str(), id, now_ms()],
        )?
    } else {
        conn.execute(
            "DELETE FROM favorites WHERE kind = ?1 AND item_id = ?2",
            params![kind.as_str(), id],
        )?
    };
    Ok(changed > 0)
}

/// Idempotent; returns the new state.
pub fn set(conn: &Connection, kind: FavKind, id: &str, on: bool) -> anyhow::Result<bool> {
    set_changed(conn, kind, id, on)?;
    Ok(on)
}

fn ids_of(conn: &Connection, kind: FavKind) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT item_id FROM favorites WHERE kind = ?1 ORDER BY added_at DESC, rowid DESC",
    )?;
    let rows = stmt.query_map([kind.as_str()], |row| row.get(0))?;
    Ok(rows.collect::<Result<Vec<String>, _>>()?)
}

pub fn ids(conn: &Connection) -> anyhow::Result<FavoriteIds> {
    Ok(FavoriteIds {
        tracks: ids_of(conn, FavKind::Track)?,
        albums: ids_of(conn, FavKind::Album)?,
        artists: ids_of(conn, FavKind::Artist)?,
    })
}

/// Favorite tracks that are in the index, newest favorite first.
pub fn tracks(conn: &Connection) -> anyhow::Result<Vec<Track>> {
    let sql = format!(
        "SELECT {TRACK_COLUMNS} FROM favorites f JOIN tracks t ON t.id = f.item_id
         WHERE f.kind = 'track' ORDER BY f.added_at DESC, f.rowid DESC"
    );
    collect(conn, &sql, [], track_from_row)
}

/// Favorite albums that are in the index, newest favorite first.
pub fn albums(conn: &Connection) -> anyhow::Result<Vec<Album>> {
    let sql = format!(
        "SELECT {ALBUM_COLUMNS} FROM favorites f JOIN albums a ON a.id = f.item_id
         WHERE f.kind = 'album' ORDER BY f.added_at DESC, f.rowid DESC"
    );
    collect(conn, &sql, [], album_from_row)
}

/// Favorite artists that have an album in the index, newest favorite first.
pub fn artists(conn: &Connection) -> anyhow::Result<Vec<Artist>> {
    let sql = artist_select("a.artist IN (SELECT item_id FROM favorites WHERE kind = 'artist')");
    let mut resolved: HashMap<String, Artist> = collect(conn, &sql, [], artist_from_row)?
        .into_iter()
        .map(|artist| (artist.name.clone(), artist))
        .collect();
    Ok(ids_of(conn, FavKind::Artist)?
        .into_iter()
        .filter_map(|name| resolved.remove(&name))
        .collect())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::db::rekey::rekey_tracks;
    use crate::library::query;
    use crate::library::scan::tests::copy_fixtures;
    use crate::library::tests::library;

    fn conn_of(library: &crate::library::Library) -> std::sync::MutexGuard<'_, Connection> {
        library.db.0.lock().unwrap()
    }

    #[test]
    fn set_is_idempotent() {
        let (library, _covers) = library();
        let conn = conn_of(&library);
        assert!(set(&conn, FavKind::Track, "t1", true).unwrap());
        assert!(set(&conn, FavKind::Track, "t1", true).unwrap());
        assert_eq!(ids(&conn).unwrap().tracks, vec!["t1".to_string()]);
        assert!(!set_changed(&conn, FavKind::Track, "t1", true).unwrap());
        assert!(!set(&conn, FavKind::Track, "t1", false).unwrap());
        assert!(!set(&conn, FavKind::Track, "t1", false).unwrap());
        assert!(!set_changed(&conn, FavKind::Track, "t1", false).unwrap());
        assert!(ids(&conn).unwrap().tracks.is_empty());
    }

    #[test]
    fn ids_group_by_kind() {
        let (library, _covers) = library();
        let conn = conn_of(&library);
        set(&conn, FavKind::Track, "same", true).unwrap();
        set(&conn, FavKind::Album, "same", true).unwrap();
        set(&conn, FavKind::Artist, "Massive Attack", true).unwrap();
        assert_eq!(
            ids(&conn).unwrap(),
            FavoriteIds {
                tracks: vec!["same".to_string()],
                albums: vec!["same".to_string()],
                artists: vec!["Massive Attack".to_string()],
            }
        );
        set(&conn, FavKind::Album, "same", false).unwrap();
        assert!(ids(&conn).unwrap().albums.is_empty());
        assert_eq!(ids(&conn).unwrap().tracks.len(), 1);
    }

    #[test]
    fn lists_resolve_and_skip_missing() {
        let (library, _covers) = library();
        let dir = copy_fixtures(&["corrupt.flac"]);
        library.add_folder(dir.path(), &|_| {}).unwrap();
        let conn = conn_of(&library);
        let all_tracks = query::tracks_range(&conn, 0, 100).unwrap();
        let all_albums = query::albums(&conn).unwrap();
        let all_artists = query::artists(&conn).unwrap();
        assert!(all_tracks.len() >= 2 && all_albums.len() >= 2 && all_artists.len() >= 2);

        set(&conn, FavKind::Track, &all_tracks[0].id, true).unwrap();
        set(&conn, FavKind::Track, &all_tracks[1].id, true).unwrap();
        set(&conn, FavKind::Track, "gone", true).unwrap();
        set(&conn, FavKind::Album, &all_albums[0].id, true).unwrap();
        set(&conn, FavKind::Album, "gone", true).unwrap();
        set(&conn, FavKind::Artist, &all_artists[0].name, true).unwrap();
        set(&conn, FavKind::Artist, &all_artists[1].name, true).unwrap();
        set(&conn, FavKind::Artist, "Nobody", true).unwrap();
        // Order is newest first, whatever the clock says.
        conn.execute("UPDATE favorites SET added_at = rowid", [])
            .unwrap();

        let listed: Vec<String> = tracks(&conn).unwrap().into_iter().map(|t| t.id).collect();
        assert_eq!(
            listed,
            vec![all_tracks[1].id.clone(), all_tracks[0].id.clone()]
        );
        let listed: Vec<String> = albums(&conn).unwrap().into_iter().map(|a| a.id).collect();
        assert_eq!(listed, vec![all_albums[0].id.clone()]);
        let listed: Vec<String> = artists(&conn)
            .unwrap()
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(
            listed,
            vec![all_artists[1].name.clone(), all_artists[0].name.clone()]
        );
        // The ids still name everything, shown or not.
        let all = ids(&conn).unwrap();
        assert_eq!(
            (all.tracks.len(), all.albums.len(), all.artists.len()),
            (3, 2, 3)
        );
    }

    #[test]
    fn favorites_survive_an_offline_folder() {
        let (library, _covers) = library();
        let dir = copy_fixtures(&["corrupt.flac"]);
        let folder = dir.path().to_path_buf();
        library.add_folder(&folder, &|_| {}).unwrap();
        let (track, album) = {
            let conn = conn_of(&library);
            let track = query::tracks_range(&conn, 0, 1).unwrap().remove(0);
            let album = query::albums(&conn).unwrap().remove(0);
            set(&conn, FavKind::Track, &track.id, true).unwrap();
            set(&conn, FavKind::Album, &album.id, true).unwrap();
            (track, album)
        };

        // The folder becomes unreadable: moved away, as an unplugged drive would be.
        let away = folder.with_file_name(format!(
            "{}-away",
            folder.file_name().unwrap().to_string_lossy()
        ));
        std::fs::rename(&folder, &away).unwrap();
        library.rescan_folder(&folder, &|_| {}).unwrap();
        assert_eq!(library.offline_folders().unwrap(), vec![folder.clone()]);
        check_listed(&library, &track.id, &album.id);

        std::fs::rename(&away, &folder).unwrap();
        library.rescan_folder(&folder, &|_| {}).unwrap();
        assert!(library.offline_folders().unwrap().is_empty());
        check_listed(&library, &track.id, &album.id);
        assert!(Path::new(&folder).is_dir());
    }

    fn check_listed(library: &crate::library::Library, track: &str, album: &str) {
        let conn = conn_of(library);
        let all = ids(&conn).unwrap();
        assert_eq!(all.tracks, vec![track.to_string()]);
        assert_eq!(all.albums, vec![album.to_string()]);
        let listed: Vec<String> = tracks(&conn).unwrap().into_iter().map(|t| t.id).collect();
        assert_eq!(listed, vec![track.to_string()]);
        let listed: Vec<String> = albums(&conn).unwrap().into_iter().map(|a| a.id).collect();
        assert_eq!(listed, vec![album.to_string()]);
    }

    fn insert_track(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id,
                duration_ms, format)
             VALUES (?1, 'local', ?1, 'A', 'A', 'B', 'local:album', 1000, 'flac')",
            [id],
        )
        .unwrap();
    }

    fn rekey(conn: &mut Connection, pairs: &[(&str, &str)]) {
        let map: Vec<(String, String)> = pairs
            .iter()
            .map(|(old, new)| (old.to_string(), new.to_string()))
            .collect();
        let tx = conn.transaction().unwrap();
        rekey_tracks(&tx, &map).unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn rekey_updates_track_favorites() {
        let (library, _covers) = library();
        let mut conn = conn_of(&library);
        insert_track(&conn, "old-1");
        set(&conn, FavKind::Track, "old-1", true).unwrap();
        // An album favorite with the same text is not a track id.
        set(&conn, FavKind::Album, "old-1", true).unwrap();
        rekey(&mut conn, &[("old-1", "new-1")]);
        let all = ids(&conn).unwrap();
        assert_eq!(all.tracks, vec!["new-1".to_string()]);
        assert_eq!(all.albums, vec!["old-1".to_string()]);
    }
}
