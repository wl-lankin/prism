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

/// The id of the artist an album (aliased `a`) belongs to: the server artist id of a Navidrome
/// album, the name otherwise.
pub(crate) const ARTIST_ID: &str = "COALESCE(a.artist_id, a.artist)";

/// Artist rows (`Artist` field order) grouped from the albums that match `filter`. One row
/// per source and artist id, so the same name in two sources gives two rows. The cover is
/// the first album cover of that artist in that source.
pub(crate) fn artist_select(filter: &str) -> String {
    format!(
        "SELECT {ARTIST_ID} AS artist_key, a.source, MIN(a.artist) AS name, COUNT(*),
            SUM(a.track_count),
            (SELECT b.cover_id FROM albums b
             WHERE b.source = a.source AND b.cover_id IS NOT NULL
                AND ((a.artist_id IS NULL AND b.artist_id IS NULL AND b.artist = a.artist)
                    OR b.artist_id = a.artist_id)
             ORDER BY b.year IS NULL, b.year, b.title COLLATE NOCASE LIMIT 1)
         FROM albums a WHERE {filter}
         GROUP BY a.source, artist_key ORDER BY name COLLATE NOCASE, a.source, artist_key"
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
        id: row.get(0)?,
        source: Source::from_db(&row.get::<_, String>(1)?),
        name: row.get(2)?,
        album_count: row.get::<_, i64>(3)?.max(0) as u32,
        track_count: row.get::<_, Option<i64>>(4)?.unwrap_or(0).max(0) as u32,
        cover_id: row.get(5)?,
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

/// Tracks in the index, of one source or (`None`) of all.
pub fn track_count(conn: &Connection, source: Option<Source>) -> anyhow::Result<u32> {
    let count: i64 = match source {
        Some(source) => conn.query_row(
            "SELECT COUNT(*) FROM tracks WHERE source = ?1",
            [source.as_str()],
            |row| row.get(0),
        )?,
        None => conn.query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))?,
    };
    Ok(count.max(0) as u32)
}

/// A page of tracks in library order, of one source (walks `idx_tracks_source_order`) or
/// (`None`) of all (walks `idx_tracks_order`).
pub fn tracks_range(
    conn: &Connection,
    offset: u32,
    limit: u32,
    source: Option<Source>,
) -> anyhow::Result<Vec<Track>> {
    match source {
        Some(source) => {
            let sql = format!(
                "SELECT {TRACK_COLUMNS} FROM tracks t WHERE t.source = ?3
                 ORDER BY {TRACK_ORDER} LIMIT ?1 OFFSET ?2"
            );
            collect(
                conn,
                &sql,
                params![limit, offset, source.as_str()],
                track_from_row,
            )
        }
        None => {
            let sql = format!(
                "SELECT {TRACK_COLUMNS} FROM tracks t ORDER BY {TRACK_ORDER} LIMIT ?1 OFFSET ?2"
            );
            collect(conn, &sql, params![limit, offset], track_from_row)
        }
    }
}

/// Every genre in the library once (case-insensitively; the first spelling in byte order
/// wins), blank ones left out, sorted case-insensitively.
pub fn genres(conn: &Connection) -> anyhow::Result<Vec<String>> {
    collect(
        conn,
        "SELECT MIN(genre) FROM tracks WHERE TRIM(COALESCE(genre, '')) <> ''
         GROUP BY genre COLLATE NOCASE ORDER BY MIN(genre) COLLATE NOCASE",
        [],
        |row| row.get(0),
    )
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
pub(crate) mod tests {
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
        assert_eq!(track_count(&conn, None).unwrap(), 7);
        let all = tracks_range(&conn, 0, 100, None).unwrap();
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
        let page = tracks_range(&conn, 2, 2, None).unwrap();
        assert_eq!(titles(&page), ["Teardrop", "Roads"]);
        assert!(tracks_range(&conn, 50, 10, None).unwrap().is_empty());
    }

    /// A Navidrome track and its album row, written as a sync would.
    pub(crate) fn insert_navidrome(
        conn: &Connection,
        song: &str,
        title: &str,
        artist: &str,
        album: &str,
        artist_id: &str,
    ) {
        let album_id = format!("navidrome:al-{album}");
        conn.execute(
            "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id,
                duration_ms, format)
             VALUES (?1, 'navidrome', ?2, ?3, ?3, ?4, ?5, 1000, 'flac')",
            params![format!("navidrome:{song}"), title, artist, album, album_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO albums (id, source, title, artist, artist_id, track_count, duration_ms)
             VALUES (?1, 'navidrome', ?2, ?3, ?4, 1, 1000)
             ON CONFLICT(id) DO UPDATE SET track_count = track_count + 1",
            params![album_id, album, artist, format!("navidrome:{artist_id}")],
        )
        .unwrap();
    }

    #[test]
    fn track_count_by_source() {
        let db = db(&[
            (
                "Teardrop",
                "Massive Attack",
                "Mezzanine",
                1,
                1,
                1998,
                "/music",
            ),
            ("Roads", "Portishead", "Dummy", 1, 4, 1994, "/music"),
        ]);
        let conn = db.0.lock().unwrap();
        insert_navidrome(&conn, "s1", "Angel", "Massive Attack", "Mezzanine", "ar-1");
        insert_navidrome(&conn, "s2", "Glory Box", "Portishead", "Dummy", "ar-2");
        insert_navidrome(&conn, "s3", "Aria", "Bjork", "Post", "ar-3");
        assert_eq!(track_count(&conn, None).unwrap(), 5);
        assert_eq!(track_count(&conn, Some(Source::Local)).unwrap(), 2);
        assert_eq!(track_count(&conn, Some(Source::Navidrome)).unwrap(), 3);
        let remote = tracks_range(&conn, 0, 100, Some(Source::Navidrome)).unwrap();
        assert_eq!(titles(&remote), ["Aria", "Angel", "Glory Box"]);
        assert!(remote.iter().all(|t| t.source == Source::Navidrome));
        let local = tracks_range(&conn, 0, 100, Some(Source::Local)).unwrap();
        assert_eq!(titles(&local), ["Teardrop", "Roads"]);
        let page = tracks_range(&conn, 1, 1, Some(Source::Navidrome)).unwrap();
        assert_eq!(titles(&page), ["Angel"]);
        assert_eq!(tracks_range(&conn, 0, 100, None).unwrap().len(), 5);
    }

    #[test]
    fn artists_split_by_source_with_ids() {
        let db = db(&[(
            "Teardrop",
            "Massive Attack",
            "Mezzanine",
            1,
            1,
            1998,
            "/music",
        )]);
        let conn = db.0.lock().unwrap();
        insert_navidrome(&conn, "s1", "Angel", "Massive Attack", "Mezzanine", "ar-1");
        insert_navidrome(
            &conn,
            "s2",
            "Karmacoma",
            "Massive Attack",
            "Protection",
            "ar-1",
        );
        conn.execute(
            "UPDATE albums SET cover_id = 'nd-al-Protection' WHERE id = 'navidrome:al-Protection'",
            [],
        )
        .unwrap();
        let artists = artists(&conn).unwrap();
        assert_eq!(artists.len(), 2, "{artists:?}");
        let local = artists.iter().find(|a| a.source == Source::Local).unwrap();
        assert_eq!(local.id, "Massive Attack");
        assert_eq!(local.name, "Massive Attack");
        assert_eq!((local.album_count, local.track_count), (1, 1));
        assert_eq!(local.cover_id.as_deref(), Some("cover-Mezzanine"));
        let remote = artists
            .iter()
            .find(|a| a.source == Source::Navidrome)
            .unwrap();
        assert_eq!(remote.id, "navidrome:ar-1");
        assert_eq!(remote.name, "Massive Attack");
        assert_eq!((remote.album_count, remote.track_count), (2, 2));
        // The cover comes from the artist's own source, never from the local album.
        assert_eq!(remote.cover_id.as_deref(), Some("nd-al-Protection"));
        let found = search(&conn, "massive", 10).unwrap();
        assert_eq!(found.artists.len(), 2);
    }

    #[test]
    fn track_looks_up_by_id() {
        let db = sample();
        let conn = db.0.lock().unwrap();
        assert_eq!(track(&conn, "t0").unwrap().unwrap().title, "Teardrop");
        assert!(track(&conn, "nope").unwrap().is_none());
    }

    #[test]
    fn genres_are_distinct_and_sorted() {
        let db = sample();
        let conn = db.0.lock().unwrap();
        for (id, genre) in [
            ("t0", Some("Trip Hop")),
            ("t1", Some("trip hop")),
            ("t2", Some("Trip Hop")),
            ("t3", Some("ambient")),
            ("t4", Some("  ")),
            ("t5", Some("")),
            ("t6", Some("Electronic")),
        ] {
            conn.execute(
                "UPDATE tracks SET genre = ?2 WHERE id = ?1",
                params![id, genre],
            )
            .unwrap();
        }
        assert_eq!(
            genres(&conn).unwrap(),
            ["ambient", "Electronic", "Trip Hop"]
        );
        conn.execute("UPDATE tracks SET genre = NULL", []).unwrap();
        assert!(genres(&conn).unwrap().is_empty());
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

    /// Title words of the scale fixture: every pair of them names about 20 of 32k tracks.
    const WORDS: [&str; 40] = [
        "amber", "blue", "cedar", "dawn", "echo", "falling", "garden", "harbor", "iron", "jade",
        "kingdom", "lantern", "meadow", "night", "ocean", "paper", "quiet", "river", "silver",
        "thunder", "under", "velvet", "winter", "yellow", "zero", "broken", "crystal", "desert",
        "electric", "forest", "golden", "hollow", "island", "jungle", "kites", "lonely", "mirror",
        "northern", "orbit", "purple",
    ];

    /// A library database file of the size of a large real one: `navidrome` server tracks
    /// (10 per album, 2 albums per artist, artist ids set, every album with a cover) and `local`
    /// tracks (10 per album, 3 albums per artist, grouped by name). Artist names overlap
    /// between the sources, as one person's local files and server usually do.
    pub(crate) fn scale_db(dir: &std::path::Path, navidrome: usize, local: usize) -> Db {
        let db = Db::open(&dir.join("library.db")).unwrap();
        let mut conn = db.0.lock().unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute(
            "INSERT INTO folders (path, online) VALUES ('/music', 1)",
            [],
        )
        .unwrap();
        {
            let mut track = tx
                .prepare(
                    "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id,
                        track_no, disc_no, duration_ms, year, genre, format, cover_id, path,
                        folder)
                     VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7, 1, ?8, ?9, 'Rock', 'flac', ?10,
                        ?11, ?12)",
                )
                .unwrap();
            let mut album = tx
                .prepare(
                    "INSERT INTO albums (id, source, title, artist, artist_id, year, cover_id,
                        track_count, duration_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 10, 2400000)",
                )
                .unwrap();
            let title = |i: usize| format!("{} {} {i}", WORDS[i % 40], WORDS[(i / 40) % 40]);
            for (source, count, per_artist) in [("navidrome", navidrome, 2), ("local", local, 3)] {
                for a in 0..count / 10 {
                    let artist_n = a / per_artist;
                    let artist = format!("Artist {artist_n}");
                    let album_title = format!("Album {a} {}", WORDS[a % 40]);
                    let year = 1970 + (a % 50) as i32;
                    let (album_id, artist_id, cover) = if source == "navidrome" {
                        (
                            format!("navidrome:al-{a}"),
                            Some(format!("navidrome:ar-{artist_n}")),
                            format!("nd-al-{a}_cover"),
                        )
                    } else {
                        (
                            crate::library::model::album_id(&artist, &album_title),
                            None,
                            format!("local-cover-{a}"),
                        )
                    };
                    album
                        .execute(params![
                            album_id,
                            source,
                            album_title,
                            artist,
                            artist_id,
                            year,
                            cover
                        ])
                        .unwrap();
                    for t in 0..10 {
                        let i = a * 10 + t;
                        let (id, path, folder) = if source == "navidrome" {
                            (format!("navidrome:tr-{i}"), None, None)
                        } else {
                            (
                                format!("local:{i}"),
                                Some(format!("/music/{i}.flac")),
                                Some("/music"),
                            )
                        };
                        track
                            .execute(params![
                                id,
                                source,
                                title(i),
                                artist,
                                album_title,
                                album_id,
                                t as i64 + 1,
                                180_000 + (i % 120) as i64 * 1000,
                                year,
                                cover,
                                path,
                                folder
                            ])
                            .unwrap();
                    }
                }
            }
        }
        tx.commit().unwrap();
        drop(conn);
        db
    }

    /// The median wall time of 5 runs of `run`, after one warm-up run.
    pub(crate) fn median_of_5(mut run: impl FnMut()) -> std::time::Duration {
        run();
        let mut times: Vec<std::time::Duration> = (0..5)
            .map(|_| {
                let started = std::time::Instant::now();
                run();
                started.elapsed()
            })
            .collect();
        times.sort();
        times[2]
    }

    /// Timing is only meaningful in an optimized build: `cargo test --release
    /// --manifest-path src-tauri/Cargo.toml with_30k -- --nocapture`.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "timing; run with --release")]
    fn search_with_30k_tracks_under_100ms() {
        let dir = tempfile::tempdir().unwrap();
        let db = scale_db(dir.path(), 30_000, 2_000);
        let conn = db.0.lock().unwrap();
        assert_eq!(track_count(&conn, None).unwrap(), 32_000);
        // A two-word query as typed into the palette: both words as prefixes.
        let found = search(&conn, "river nigh", 50).unwrap();
        assert!(found
            .tracks
            .iter()
            .any(|t| t.title.contains("river") && t.title.contains("night")));
        let two_words = median_of_5(|| {
            search(&conn, "river nigh", 50).unwrap();
        });
        // The worst case: one word that is in every artist name, so every row matches.
        let every_row = median_of_5(|| {
            search(&conn, "artist", 50).unwrap();
        });
        println!("search, 30k + 2k tracks: two words {two_words:?}, every row {every_row:?}");
        assert!(two_words.as_millis() < 100, "two words took {two_words:?}");
        assert!(every_row.as_millis() < 100, "every row took {every_row:?}");
    }

    #[test]
    #[cfg_attr(debug_assertions, ignore = "timing; run with --release")]
    fn songs_page_with_30k_under_20ms() {
        let dir = tempfile::tempdir().unwrap();
        let db = scale_db(dir.path(), 30_000, 2_000);
        let conn = db.0.lock().unwrap();
        let page = tracks_range(&conn, 15_000, 200, Some(Source::Navidrome)).unwrap();
        assert_eq!(page.len(), 200);
        assert!(page.iter().all(|t| t.source == Source::Navidrome));
        let navidrome = median_of_5(|| {
            tracks_range(&conn, 15_000, 200, Some(Source::Navidrome)).unwrap();
        });
        let all = median_of_5(|| {
            tracks_range(&conn, 15_000, 200, None).unwrap();
        });
        println!("songs page at 15000, 30k + 2k tracks: navidrome {navidrome:?}, all {all:?}");
        assert!(
            navidrome.as_millis() < 20,
            "navidrome page took {navidrome:?}"
        );
        assert!(all.as_millis() < 20, "all-sources page took {all:?}");
    }

    /// The artist list over 30k server and 30k local tracks: the whole list, one source's
    /// list, and the artist part of a search.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "timing; run with --release")]
    fn artists_with_30k_and_30k_under_100ms() {
        let dir = tempfile::tempdir().unwrap();
        let db = scale_db(dir.path(), 30_000, 30_000);
        let conn = db.0.lock().unwrap();
        let listed = artists(&conn).unwrap();
        // 1500 server artists (2 albums each) and 1000 local ones (3 albums each).
        assert_eq!(listed.len(), 2_500);
        assert!(listed.iter().all(|a| a.cover_id.is_some()));
        let by_source = |source: &str| {
            let sql = artist_select("a.source = ?1");
            collect(&conn, &sql, [source], artist_from_row).unwrap()
        };
        assert_eq!(by_source("navidrome").len(), 1_500);
        assert_eq!(by_source("local").len(), 1_000);
        let all = median_of_5(|| {
            artists(&conn).unwrap();
        });
        let navidrome = median_of_5(|| {
            by_source("navidrome");
        });
        let local = median_of_5(|| {
            by_source("local");
        });
        println!(
            "artists, 30k + 30k tracks: all {all:?}, navidrome {navidrome:?}, local {local:?}"
        );
        assert!(all.as_millis() < 100, "all artists took {all:?}");
        assert!(
            navidrome.as_millis() < 100,
            "navidrome artists took {navidrome:?}"
        );
        assert!(local.as_millis() < 100, "local artists took {local:?}");
    }

    #[test]
    fn fts_query_quotes_every_word() {
        assert_eq!(fts_query("glo bo").as_deref(), Some("\"glo\"* \"bo\"*"));
        assert_eq!(fts_query("a\"b").as_deref(), Some("\"a\"\"b\"*"));
        assert_eq!(fts_query("  - * "), None);
    }
}
