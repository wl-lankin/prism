//! Read queries behind the `library_*` commands.

use rusqlite::{params, Connection, OptionalExtension, Params, Row};
use serde::Serialize;

use super::model::{Album, Artist, Folder, Source, Track};

/// Result of `library_search`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub tracks: Vec<Track>,
}

/// Every `Track` field, from a `tracks` table aliased `t`, in `track_from_row` order. A track
/// without its own cover shows the album's online lookup cover; COALESCE is lazy and the
/// subquery is a primary key search, so tracks with a cover cost nothing extra.
pub(crate) const TRACK_COLUMNS: &str =
    "t.id, t.source, t.title, t.artist, t.album_artist, t.album, \
    t.album_id, t.track_no, t.disc_no, t.duration_ms, t.year, t.genre, t.format, \
    t.sample_rate, t.bit_depth, t.bitrate_kbps, \
    COALESCE(t.cover_id, (SELECT l.cover_id FROM cover_lookups l WHERE l.album_id = t.album_id)), \
    t.path, t.replay_gain_track_db, \
    t.replay_gain_album_db, t.replay_gain_track_peak, t.replay_gain_album_peak";

/// Every `Album` field, from an `albums` table aliased `a`, in `album_from_row` order.
pub(crate) const ALBUM_COLUMNS: &str =
    "a.id, a.source, a.title, a.artist, a.year, a.cover_id, a.track_count, a.duration_ms";

pub(crate) const ALBUM_ORDER: &str =
    "a.artist COLLATE NOCASE, a.year IS NULL, a.year, a.title COLLATE NOCASE, a.id";

pub(crate) const TRACK_ORDER: &str = "t.artist COLLATE NOCASE, t.album COLLATE NOCASE, \
    t.disc_no IS NULL, t.disc_no, t.track_no IS NULL, t.track_no, t.title COLLATE NOCASE, t.id";

/// Artist rows (`Artist` field order) grouped from the albums that match `filter`.
pub(crate) fn artist_select(filter: &str) -> String {
    format!(
        "SELECT a.artist, COUNT(*), SUM(a.track_count),
            (SELECT b.cover_id FROM albums b
             WHERE b.artist = a.artist AND b.cover_id IS NOT NULL
             ORDER BY b.year IS NULL, b.year, b.title COLLATE NOCASE LIMIT 1)
         FROM albums a WHERE {filter}
         GROUP BY a.artist ORDER BY a.artist COLLATE NOCASE"
    )
}

pub(crate) fn track_from_row(row: &Row) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get(0)?,
        source: Source::from_db(&row.get::<_, String>(1)?),
        title: row.get(2)?,
        artist: row.get(3)?,
        album_artist: row.get(4)?,
        album: row.get(5)?,
        album_id: row.get(6)?,
        track_no: row.get(7)?,
        disc_no: row.get(8)?,
        duration_ms: row.get::<_, i64>(9)?.max(0) as u64,
        year: row.get(10)?,
        genre: row.get(11)?,
        format: row.get(12)?,
        sample_rate: row.get(13)?,
        bit_depth: row.get(14)?,
        bitrate_kbps: row.get(15)?,
        cover_id: row.get(16)?,
        path: row.get(17)?,
        replay_gain_track_db: row.get(18)?,
        replay_gain_album_db: row.get(19)?,
        replay_gain_track_peak: row.get(20)?,
        replay_gain_album_peak: row.get(21)?,
    })
}

pub(crate) fn album_from_row(row: &Row) -> rusqlite::Result<Album> {
    Ok(Album {
        id: row.get(0)?,
        source: Source::from_db(&row.get::<_, String>(1)?),
        title: row.get(2)?,
        artist: row.get(3)?,
        year: row.get(4)?,
        cover_id: row.get(5)?,
        track_count: row.get::<_, i64>(6)?.max(0) as u32,
        duration_ms: row.get::<_, i64>(7)?.max(0) as u64,
    })
}

pub(crate) fn artist_from_row(row: &Row) -> rusqlite::Result<Artist> {
    Ok(Artist {
        name: row.get(0)?,
        album_count: row.get::<_, i64>(1)?.max(0) as u32,
        track_count: row.get::<_, Option<i64>>(2)?.unwrap_or(0).max(0) as u32,
        cover_id: row.get(3)?,
    })
}

pub(crate) fn collect<T, P: Params>(
    conn: &Connection,
    sql: &str,
    params: P,
    map: fn(&Row) -> rusqlite::Result<T>,
) -> anyhow::Result<Vec<T>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params, map)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn folders(conn: &Connection) -> anyhow::Result<Vec<Folder>> {
    collect(
        conn,
        "SELECT f.path, f.online, (SELECT COUNT(*) FROM tracks t WHERE t.folder = f.path)
         FROM folders f ORDER BY f.rowid",
        [],
        |row| {
            Ok(Folder {
                path: row.get(0)?,
                online: row.get::<_, i64>(1)? != 0,
                track_count: row.get::<_, i64>(2)?.max(0) as u32,
            })
        },
    )
}

pub fn track(conn: &Connection, id: &str) -> anyhow::Result<Option<Track>> {
    let sql = format!("SELECT {TRACK_COLUMNS} FROM tracks t WHERE t.id = ?1");
    Ok(conn.query_row(&sql, [id], track_from_row).optional()?)
}

pub fn albums(conn: &Connection) -> anyhow::Result<Vec<Album>> {
    let sql = format!("SELECT {ALBUM_COLUMNS} FROM albums a ORDER BY {ALBUM_ORDER}");
    collect(conn, &sql, [], album_from_row)
}

pub fn album_tracks(conn: &Connection, album_id: &str) -> anyhow::Result<Vec<Track>> {
    let sql = format!(
        "SELECT {TRACK_COLUMNS} FROM tracks t WHERE t.album_id = ?1
         ORDER BY t.disc_no IS NULL, t.disc_no, t.track_no IS NULL, t.track_no,
            t.title COLLATE NOCASE, t.path"
    );
    collect(conn, &sql, [album_id], track_from_row)
}

pub fn artists(conn: &Connection) -> anyhow::Result<Vec<Artist>> {
    collect(conn, &artist_select("1"), [], artist_from_row)
}

pub fn artist_albums(conn: &Connection, name: &str) -> anyhow::Result<Vec<Album>> {
    let sql = format!(
        "SELECT {ALBUM_COLUMNS} FROM albums a WHERE a.artist = ?1
         ORDER BY a.year IS NULL, a.year, a.title COLLATE NOCASE, a.id"
    );
    collect(conn, &sql, [name], album_from_row)
}

pub fn track_count(conn: &Connection) -> anyhow::Result<u32> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))?;
    Ok(count.max(0) as u32)
}

pub fn tracks_range(conn: &Connection, offset: u32, limit: u32) -> anyhow::Result<Vec<Track>> {
    let sql =
        format!("SELECT {TRACK_COLUMNS} FROM tracks t ORDER BY {TRACK_ORDER} LIMIT ?1 OFFSET ?2");
    collect(conn, &sql, params![limit, offset], track_from_row)
}

/// Tracks by FTS (every word as a prefix), albums and artists by substring. Any input is
/// safe: FTS syntax is quoted away and LIKE wildcards are escaped.
pub fn search(conn: &Connection, query: &str, limit: u32) -> anyhow::Result<SearchResult> {
    let query = query.trim();
    if query.is_empty() || limit == 0 {
        return Ok(SearchResult::default());
    }
    let pattern = like_pattern(query);
    let albums_sql = format!(
        "SELECT {ALBUM_COLUMNS} FROM albums a
         WHERE a.title LIKE ?1 ESCAPE '\\' OR a.artist LIKE ?1 ESCAPE '\\'
         ORDER BY {ALBUM_ORDER} LIMIT ?2"
    );
    let albums = collect(conn, &albums_sql, params![pattern, limit], album_from_row)?;
    let artists_sql = format!("{} LIMIT ?2", artist_select("a.artist LIKE ?1 ESCAPE '\\'"));
    let artists = collect(conn, &artists_sql, params![pattern, limit], artist_from_row)?;
    let tracks = match fts_query(query) {
        Some(fts) => {
            let sql = format!(
                "SELECT {TRACK_COLUMNS} FROM tracks_fts f JOIN tracks t ON t.row_id = f.rowid
                 WHERE tracks_fts MATCH ?1 ORDER BY f.rank, {TRACK_ORDER} LIMIT ?2"
            );
            collect(conn, &sql, params![fts, limit], track_from_row)?
        }
        None => Vec::new(),
    };
    Ok(SearchResult {
        albums,
        artists,
        tracks,
    })
}

/// Turns user input into a safe FTS5 query: every word becomes a quoted prefix term
/// (`"word"*`), so quotes, operators and column filters are taken literally. Words without
/// a letter or digit are dropped (they would be empty phrases).
pub fn fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .map(|word| format!("\"{}\"*", word.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

/// `%input%` with LIKE wildcards and the escape character escaped.
fn like_pattern(input: &str) -> String {
    let mut pattern = String::from("%");
    for c in input.chars() {
        if matches!(c, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern.push('%');
    pattern
}

#[cfg(test)]
mod tests {
    use rusqlite::params;

    use super::*;
    use crate::db::Db;
    use crate::library::scan::rebuild_albums;

    /// (title, artist/album artist, album, disc, track, year, folder)
    type Row<'a> = (&'a str, &'a str, &'a str, u32, u32, i32, &'a str);

    fn db(rows: &[Row]) -> Db {
        let db = Db::open_in_memory().unwrap();
        {
            let conn = db.0.lock().unwrap();
            for folder in ["/music", "/other"] {
                conn.execute(
                    "INSERT INTO folders (path, online) VALUES (?1, ?2)",
                    params![folder, (folder == "/music") as i64],
                )
                .unwrap();
            }
            let mut albums = Vec::new();
            for (i, (title, artist, album, disc, track, year, folder)) in rows.iter().enumerate() {
                let album_id = crate::library::model::album_id(artist, album);
                conn.execute(
                    "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id,
                        track_no, disc_no, duration_ms, year, format, path, folder, cover_id)
                     VALUES (?1, 'local', ?2, ?3, ?3, ?4, ?5, ?6, ?7, 1000, ?8, 'flac', ?9, ?10, ?11)",
                    params![
                        format!("t{i}"),
                        title,
                        artist,
                        album,
                        album_id,
                        track,
                        disc,
                        year,
                        format!("{folder}/{i}.flac"),
                        folder,
                        format!("cover-{album}")
                    ],
                )
                .unwrap();
                albums.push(album_id);
            }
            rebuild_albums(&conn, albums.iter()).unwrap();
        }
        db
    }

    fn sample() -> Db {
        db(&[
            (
                "Teardrop",
                "Massive Attack",
                "Mezzanine",
                1,
                3,
                1998,
                "/music",
            ),
            ("Angel", "Massive Attack", "Mezzanine", 1, 1, 1998, "/music"),
            (
                "Unfinished Sympathy",
                "Massive Attack",
                "Blue Lines",
                1,
                5,
                1991,
                "/music",
            ),
            ("Glory Box", "Portishead", "Dummy", 1, 11, 1994, "/music"),
            ("Roads", "Portishead", "Dummy", 1, 4, 1994, "/music"),
            ("Second Disc", "Portishead", "Dummy", 2, 1, 1994, "/other"),
            (
                "Everything In Its Right Place",
                "Radiohead",
                "Kid A",
                1,
                1,
                2000,
                "/other",
            ),
        ])
    }

    fn titles(tracks: &[Track]) -> Vec<&str> {
        tracks.iter().map(|t| t.title.as_str()).collect()
    }

    #[test]
    fn folders_report_online_state_and_track_counts() {
        let db = sample();
        let conn = db.0.lock().unwrap();
        let folders = folders(&conn).unwrap();
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].path, "/music");
        assert!(folders[0].online);
        assert_eq!(folders[0].track_count, 5);
        assert!(!folders[1].online);
        assert_eq!(folders[1].track_count, 2);
    }

    #[test]
    fn albums_sort_by_artist_year_title() {
        let db = sample();
        let conn = db.0.lock().unwrap();
        let albums = albums(&conn).unwrap();
        let names: Vec<&str> = albums.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(names, ["Blue Lines", "Mezzanine", "Dummy", "Kid A"]);
        assert_eq!(albums[2].track_count, 3);
    }

    #[test]
    fn album_tracks_follow_disc_and_track_numbers() {
        let db = sample();
        let conn = db.0.lock().unwrap();
        let album = crate::library::model::album_id("Portishead", "Dummy");
        let tracks = album_tracks(&conn, &album).unwrap();
        assert_eq!(titles(&tracks), ["Roads", "Glory Box", "Second Disc"]);
        assert_eq!(tracks[0].path.as_deref(), Some("/music/4.flac"));
        assert!(album_tracks(&conn, "nope").unwrap().is_empty());
    }

    #[test]
    fn artists_aggregate_their_albums() {
        let db = sample();
        let conn = db.0.lock().unwrap();
        let artists = artists(&conn).unwrap();
        let names: Vec<&str> = artists.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Massive Attack", "Portishead", "Radiohead"]);
        assert_eq!(artists[0].album_count, 2);
        assert_eq!(artists[0].track_count, 3);
        assert_eq!(artists[0].cover_id.as_deref(), Some("cover-Blue Lines"));
        let albums = artist_albums(&conn, "Massive Attack").unwrap();
        let names: Vec<&str> = albums.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(names, ["Blue Lines", "Mezzanine"]);
    }

    #[test]
    fn tracks_range_pages_in_library_order() {
        let db = sample();
        let conn = db.0.lock().unwrap();
        assert_eq!(track_count(&conn).unwrap(), 7);
        let all = tracks_range(&conn, 0, 100).unwrap();
        assert_eq!(
            titles(&all),
            [
                "Unfinished Sympathy",
                "Angel",
                "Teardrop",
                "Roads",
                "Glory Box",
                "Second Disc",
                "Everything In Its Right Place"
            ]
        );
        let page = tracks_range(&conn, 2, 2).unwrap();
        assert_eq!(titles(&page), ["Teardrop", "Roads"]);
        assert!(tracks_range(&conn, 50, 10).unwrap().is_empty());
    }

    #[test]
    fn track_looks_up_by_id() {
        let db = sample();
        let conn = db.0.lock().unwrap();
        assert_eq!(track(&conn, "t0").unwrap().unwrap().title, "Teardrop");
        assert!(track(&conn, "nope").unwrap().is_none());
    }

    #[test]
    fn search_matches_word_prefixes_albums_and_artists() {
        let db = sample();
        let conn = db.0.lock().unwrap();
        let result = search(&conn, "tear", 10).unwrap();
        assert_eq!(titles(&result.tracks), ["Teardrop"]);
        let result = search(&conn, "glo bo", 10).unwrap();
        assert_eq!(titles(&result.tracks), ["Glory Box"]);
        let result = search(&conn, "porti", 10).unwrap();
        assert_eq!(result.artists.len(), 1);
        assert_eq!(result.artists[0].name, "Portishead");
        assert_eq!(result.albums.len(), 1);
        assert_eq!(result.tracks.len(), 3);
        let result = search(&conn, "mezz", 10).unwrap();
        assert_eq!(result.albums[0].title, "Mezzanine");
        let result = search(&conn, "massive", 2).unwrap();
        assert_eq!(result.tracks.len(), 2, "the limit applies");
    }

    #[test]
    fn search_is_safe_for_any_input() {
        let db = sample();
        let conn = db.0.lock().unwrap();
        for input in [
            "\"",
            "\"tear",
            "tear\"drop",
            "AND",
            "a OR",
            "NOT b",
            "NEAR(",
            "title:tear",
            "*",
            "tear*",
            "(",
            ")",
            "-",
            "^",
            "'",
            "%",
            "_",
            "\\",
            "",
            "   ",
            "{x}",
            "a + b",
        ] {
            search(&conn, input, 10).unwrap_or_else(|err| panic!("{input:?}: {err:#}"));
        }
        assert_eq!(
            titles(&search(&conn, "\"tear", 10).unwrap().tracks),
            ["Teardrop"]
        );
        assert_eq!(
            titles(&search(&conn, "tear*", 10).unwrap().tracks),
            ["Teardrop"]
        );
        assert!(
            search(&conn, "%", 10).unwrap().albums.is_empty(),
            "% is literal"
        );
        assert_eq!(search(&conn, "", 10).unwrap(), SearchResult::default());
    }

    #[test]
    fn fts_query_quotes_every_word() {
        assert_eq!(fts_query("glo bo").as_deref(), Some("\"glo\"* \"bo\"*"));
        assert_eq!(fts_query("a\"b").as_deref(), Some("\"a\"\"b\"*"));
        assert_eq!(fts_query("  - * "), None);
    }
}
