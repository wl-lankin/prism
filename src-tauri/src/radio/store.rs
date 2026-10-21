//! SQL for `radio_stations`, `radio_favicons` and `radio_state`. Plain functions over a
//! connection; the caller holds the DB lock and never does network work under it.

use rusqlite::{params, Connection, OptionalExtension, Row};

use super::model::{station_cover_id, Station};
use crate::library::model::Track;

pub(crate) const STATION_COLUMNS: &str = "s.id, s.name, s.url, s.url_resolved, s.homepage, \
     s.favicon, s.cover_id, s.country_code, s.region, s.language, s.tags, s.codec, \
     s.bitrate_kbps, s.hls, s.last_check_ok";

pub(crate) fn station_from_row(row: &Row) -> rusqlite::Result<Station> {
    let tags: String = row.get(10)?;
    Ok(Station {
        id: row.get(0)?,
        name: row.get(1)?,
        url: row.get(2)?,
        url_resolved: row.get(3)?,
        homepage: row.get(4)?,
        favicon: row.get(5)?,
        cover_id: row.get(6)?,
        country_code: row.get(7)?,
        region: row.get(8)?,
        language: row.get(9)?,
        tags: tags
            .split(',')
            .filter(|tag| !tag.is_empty())
            .map(str::to_string)
            .collect(),
        codec: row.get(11)?,
        bitrate_kbps: row.get(12)?,
        hls: row.get::<_, i64>(13)? != 0,
        last_check_ok: row.get::<_, i64>(14)? != 0,
        distance_km: None,
    })
}

/// Where the logo of a cover id is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaviconSource {
    pub url: Option<String>,
    pub homepage: Option<String>,
}

/// Inserts a station, or updates every field the directory owns. `saved_at`,
/// `last_played_at` and `last_click_at` of an existing row stay. The logo is remembered too,
/// so the station's tile loads.
pub fn upsert_station(conn: &Connection, station: &Station, now_ms: i64) -> anyhow::Result<()> {
    let cover_id = station
        .cover_id
        .clone()
        .or_else(|| station_cover_id(station.favicon.as_deref(), station.homepage.as_deref()));
    conn.execute(
        "INSERT INTO radio_stations (id, name, url, url_resolved, homepage, favicon, cover_id,
            country_code, region, language, tags, codec, bitrate_kbps, hls, last_check_ok,
            saved_at, refreshed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16)
         ON CONFLICT(id) DO UPDATE SET
            name = excluded.name, url = excluded.url, url_resolved = excluded.url_resolved,
            homepage = excluded.homepage, favicon = excluded.favicon,
            cover_id = excluded.cover_id, country_code = excluded.country_code,
            region = excluded.region, language = excluded.language, tags = excluded.tags,
            codec = excluded.codec, bitrate_kbps = excluded.bitrate_kbps, hls = excluded.hls,
            last_check_ok = excluded.last_check_ok, refreshed_at = excluded.refreshed_at",
        params![
            station.id,
            station.name,
            station.url,
            station.url_resolved,
            station.homepage,
            station.favicon,
            cover_id,
            station.country_code,
            station.region,
            station.language,
            station.tags.join(","),
            station.codec,
            station.bitrate_kbps,
            station.hls as i64,
            station.last_check_ok as i64,
            now_ms,
        ],
    )?;
    remember_favicons(conn, std::slice::from_ref(station))
}

pub fn station(conn: &Connection, id: &str) -> anyhow::Result<Option<Station>> {
    let sql = format!("SELECT {STATION_COLUMNS} FROM radio_stations s WHERE s.id = ?1");
    Ok(conn.query_row(&sql, [id], station_from_row).optional()?)
}

/// A stored station as the player's track.
pub fn track(conn: &Connection, id: &str) -> anyhow::Result<Option<Track>> {
    Ok(station(conn, id)?.map(|station| station.as_track()))
}

/// Writes where each station's logo comes from. A station without a usable favicon or
/// homepage has no logo and writes nothing. An existing row keeps its `missing_until`.
pub fn remember_favicons(conn: &Connection, stations: &[Station]) -> anyhow::Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO radio_favicons (cover_id, url, homepage) VALUES (?1, ?2, ?3)
         ON CONFLICT(cover_id) DO UPDATE SET
            url = COALESCE(excluded.url, url), homepage = COALESCE(excluded.homepage, homepage)",
    )?;
    for station in stations {
        let favicon = station.favicon.as_deref();
        let homepage = station.homepage.as_deref();
        let Some(cover_id) = station_cover_id(favicon, homepage) else {
            continue;
        };
        let url = favicon.filter(|url| super::model::favicon_cover_id(url).is_some());
        let homepage = homepage.filter(|home| super::model::homepage_cover_id(home).is_some());
        stmt.execute(params![cover_id, url, homepage])?;
    }
    Ok(())
}

pub fn favicon_source(conn: &Connection, cover_id: &str) -> anyhow::Result<Option<FaviconSource>> {
    Ok(conn
        .query_row(
            "SELECT url, homepage FROM radio_favicons WHERE cover_id = ?1",
            [cover_id],
            |row| {
                Ok(FaviconSource {
                    url: row.get(0)?,
                    homepage: row.get(1)?,
                })
            },
        )
        .optional()?)
}

pub fn mark_favicon_missing(
    conn: &Connection,
    cover_id: &str,
    until_ms: i64,
) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE radio_favicons SET missing_until = ?2 WHERE cover_id = ?1",
        params![cover_id, until_ms],
    )?;
    Ok(())
}

pub fn mark_played(conn: &Connection, id: &str, now_ms: i64) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE radio_stations SET last_played_at = ?2 WHERE id = ?1",
        params![id, now_ms],
    )?;
    Ok(())
}

/// The most recently played stations that are not favorites (those are listed on their own),
/// newest first.
pub fn recent(conn: &Connection, limit: usize) -> anyhow::Result<Vec<Station>> {
    let sql = format!(
        "SELECT {STATION_COLUMNS} FROM radio_stations s
         WHERE s.last_played_at IS NOT NULL AND {NOT_FAVORITE}
         ORDER BY s.last_played_at DESC, s.rowid DESC LIMIT ?1"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([limit as i64], station_from_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

const NOT_FAVORITE: &str = "NOT EXISTS (SELECT 1 FROM favorites f
     WHERE f.kind = 'station' AND f.item_id = s.id)";

/// Deletes the rows that are neither a favorite nor among the newest `keep_recent` played
/// stations; returns how many went. Logo rows stay: search tiles use them.
pub fn prune(conn: &Connection, keep_recent: usize) -> anyhow::Result<usize> {
    let sql = format!(
        "DELETE FROM radio_stations WHERE id IN (
            SELECT s.id FROM radio_stations s
            WHERE {NOT_FAVORITE}
              AND s.id NOT IN (
                SELECT s.id FROM radio_stations s
                WHERE s.last_played_at IS NOT NULL AND {NOT_FAVORITE}
                ORDER BY s.last_played_at DESC, s.rowid DESC LIMIT ?1))"
    );
    Ok(conn.execute(&sql, [keep_recent as i64])?)
}

pub fn state_get(conn: &Connection, key: &str) -> anyhow::Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM radio_state WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn state_set(conn: &Connection, key: &str, value: &str) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO radio_state (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::library::favorites::{self, FavKind};
    use crate::library::model::Source;
    use crate::library::query;
    use crate::radio::model::tests::station;
    use crate::radio::model::{favicon_cover_id, homepage_cover_id};

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    }

    fn favorite(conn: &Connection, station: &Station) {
        assert!(favorites::set_with_source(
            conn,
            FavKind::Station,
            &station.id,
            true,
            Source::Radio,
            1
        )
        .unwrap());
    }

    #[test]
    fn radio_tables_exist() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        for table in ["radio_stations", "radio_favicons", "radio_state"] {
            assert_eq!(count(&conn, table), 0, "{table}");
        }
        let index: String = conn
            .query_row(
                "SELECT tbl_name FROM sqlite_master WHERE name = 'idx_radio_stations_played'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index, "radio_stations");
    }

    #[test]
    fn upsert_round_trips_and_keeps_play_history() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        let mut one = station(1);
        one.favicon = Some("https://example.test/logo.png".to_string());
        one.homepage = Some("https://example.test/".to_string());
        one.tags = vec!["pop".to_string(), "rock".to_string()];
        one.cover_id = favicon_cover_id("https://example.test/logo.png");
        upsert_station(&conn, &one, 100).unwrap();
        assert_eq!(station_of(&conn, &one.id), one);
        mark_played(&conn, &one.id, 150).unwrap();

        let mut changed = one.clone();
        changed.name = "Renamed".to_string();
        changed.url = "http://127.0.0.1:1/new".to_string();
        upsert_station(&conn, &changed, 200).unwrap();
        assert_eq!(station_of(&conn, &one.id), changed);
        let (saved, refreshed, played): (i64, i64, i64) = conn
            .query_row(
                "SELECT saved_at, refreshed_at, last_played_at FROM radio_stations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((saved, refreshed, played), (100, 200, 150));
        assert_eq!(count(&conn, "radio_stations"), 1);
        assert_eq!(super::station(&conn, "radio:nobody").unwrap(), None);
    }

    fn station_of(conn: &Connection, id: &str) -> Station {
        super::station(conn, id).unwrap().unwrap()
    }

    #[test]
    fn track_resolves_a_stored_station() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        let one = station(1);
        upsert_station(&conn, &one, 1).unwrap();
        let resolved = track(&conn, &one.id).unwrap().unwrap();
        assert_eq!(resolved, one.as_track());
        assert_eq!(track(&conn, "radio:none").unwrap(), None);
    }

    #[test]
    fn favicons_are_remembered_with_url_and_homepage() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        let mut with_icon = station(1);
        with_icon.favicon = Some("https://example.test/logo.png".to_string());
        with_icon.homepage = Some("https://example.test/".to_string());
        let mut only_home = station(2);
        only_home.homepage = Some("https://home.test/".to_string());
        let bare = station(3);
        let mut bad = station(4);
        bad.favicon = Some("ftp://example.test/logo.png".to_string());
        remember_favicons(&conn, &[with_icon.clone(), only_home, bare, bad]).unwrap();
        assert_eq!(count(&conn, "radio_favicons"), 2);

        let id = favicon_cover_id("https://example.test/logo.png").unwrap();
        assert_eq!(
            favicon_source(&conn, &id).unwrap(),
            Some(FaviconSource {
                url: Some("https://example.test/logo.png".to_string()),
                homepage: Some("https://example.test/".to_string()),
            })
        );
        let home_id = homepage_cover_id("https://home.test/").unwrap();
        assert_eq!(
            favicon_source(&conn, &home_id).unwrap(),
            Some(FaviconSource {
                url: None,
                homepage: Some("https://home.test/".to_string()),
            })
        );
        assert_eq!(favicon_source(&conn, "rb-unknown").unwrap(), None);

        mark_favicon_missing(&conn, &id, 999).unwrap();
        // Remembering again (a new search) keeps the missing mark.
        remember_favicons(&conn, &[with_icon]).unwrap();
        let until: i64 = conn
            .query_row(
                "SELECT missing_until FROM radio_favicons WHERE cover_id = ?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(until, 999);
    }

    #[test]
    fn upsert_remembers_the_logo_of_a_stored_station() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        let mut one = station(1);
        one.homepage = Some("https://home.test/".to_string());
        upsert_station(&conn, &one, 1).unwrap();
        let cover = station_of(&conn, &one.id).cover_id.unwrap();
        assert_eq!(cover, homepage_cover_id("https://home.test/").unwrap());
        assert!(favicon_source(&conn, &cover).unwrap().is_some());
    }

    #[test]
    fn prune_keeps_favorites_and_recent() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        for n in 1..=8 {
            upsert_station(&conn, &station(n), 1).unwrap();
        }
        // 1..=6 played in order, 7 saved without a play, 8 never touched.
        for n in 1..=6 {
            mark_played(&conn, &station(n).id, 10 + n as i64).unwrap();
        }
        favorite(&conn, &station(2));
        favorite(&conn, &station(7));

        let listed: Vec<String> = recent(&conn, 10)
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        let expected: Vec<String> = [6, 5, 4, 3, 1].iter().map(|n| station(*n).id).collect();
        assert_eq!(listed, expected);

        // Newest 2 non-favorites (6, 5) stay, favorites 2 and 7 stay; 1, 3, 4 and 8 go.
        assert_eq!(prune(&conn, 2).unwrap(), 4);
        let mut left: Vec<String> = conn
            .prepare("SELECT id FROM radio_stations")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        left.sort();
        let mut want: Vec<String> = [2, 5, 6, 7].iter().map(|n| station(*n).id).collect();
        want.sort();
        assert_eq!(left, want);
        assert_eq!(prune(&conn, 2).unwrap(), 0);
        assert_eq!(recent(&conn, 1).unwrap().len(), 1);
        assert_eq!(prune(&conn, 0).unwrap(), 2);
        assert_eq!(count(&conn, "radio_stations"), 2);
    }

    #[test]
    fn state_round_trips() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        assert_eq!(state_get(&conn, "place").unwrap(), None);
        state_set(&conn, "place", "one").unwrap();
        state_set(&conn, "place", "two").unwrap();
        assert_eq!(state_get(&conn, "place").unwrap().as_deref(), Some("two"));
    }

    #[test]
    fn station_favorites_list_and_resolve() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        for n in 1..=3 {
            upsert_station(&conn, &station(n), 1).unwrap();
        }
        favorite(&conn, &station(1));
        assert!(favorites::set_with_source(
            &conn,
            FavKind::Station,
            &station(3).id,
            true,
            Source::Radio,
            5
        )
        .unwrap());
        // A favorite whose station row is not stored is kept in the ids, not listed.
        assert!(favorites::set_with_source(
            &conn,
            FavKind::Station,
            "radio:gone",
            true,
            Source::Radio,
            3
        )
        .unwrap());

        let listed: Vec<String> = favorites::stations(&conn)
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(listed, vec![station(3).id, station(1).id]);
        let ids = favorites::ids(&conn).unwrap();
        assert_eq!(
            ids.stations,
            vec![station(3).id, "radio:gone".into(), station(1).id]
        );
        assert!(ids.tracks.is_empty() && ids.albums.is_empty() && ids.artists.is_empty());
        let source: String = conn
            .query_row(
                "SELECT source FROM favorites WHERE item_id = ?1",
                [station(1).id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source, "radio");

        assert!(favorites::set_changed(&conn, FavKind::Station, &station(1).id, false).unwrap());
        assert_eq!(favorites::stations(&conn).unwrap().len(), 1);
    }

    #[test]
    fn stations_never_touch_tracks() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        let before = query::track_count(&conn, None).unwrap();
        for n in 1..=10 {
            upsert_station(&conn, &station(n), 1).unwrap();
        }
        for n in 1..=3 {
            favorite(&conn, &station(n));
        }
        mark_played(&conn, &station(4).id, 5).unwrap();
        for table in ["tracks", "albums", "tracks_fts", "track_stats"] {
            assert_eq!(count(&conn, table), 0, "{table}");
        }
        assert_eq!(query::track_count(&conn, None).unwrap(), before);
        assert_eq!(query::track_count(&conn, Some(Source::Radio)).unwrap(), 0);
    }
}
