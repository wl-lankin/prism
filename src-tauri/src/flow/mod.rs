//! Flow: an endless queue dealt from a pool of tracks (favorites, the library or one genre).

pub mod deck;

use std::collections::HashSet;

use rusqlite::{params_from_iter, Connection};
use serde::{Deserialize, Serialize};

pub use deck::Deck;

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
/// artist (exact name). A pool under `PAD_TARGET` is padded from the library: first tracks
/// by the pool's artists, then tracks of the pool's genres.
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
    if found.len() < PAD_TARGET {
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
    fn library_pool_holds_every_track() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        add(&conn, "a", "A", "A", "x", None);
        add(&conn, "b", "B", "B", "y", Some("Jazz"));
        let pool = pool(&conn, &FlowVariant::Library).unwrap();
        assert_eq!(ids(&pool), vec!["a", "b"]);
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
