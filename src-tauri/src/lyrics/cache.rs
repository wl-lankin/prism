//! The `lyrics_cache` table: found lyrics are kept, "nothing found" is remembered for a week.

use rusqlite::{params, Connection, OptionalExtension};

use super::lrclib::Query;
use super::Lyrics;

/// How long a stored miss counts, in seconds.
pub const MISS_TTL_SECS: i64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cached {
    /// `None` is a remembered miss that has not expired.
    Hit(Option<Lyrics>),
    Miss,
}

pub fn get(conn: &Connection, key: &str, now: i64) -> anyhow::Result<Cached> {
    let row: Option<(Option<String>, i64)> = conn
        .query_row(
            "SELECT lyrics, checked_at FROM lyrics_cache WHERE key = ?1",
            params![key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(match row {
        None => Cached::Miss,
        Some((None, checked_at)) if now - checked_at <= MISS_TTL_SECS => Cached::Hit(None),
        Some((None, _)) => Cached::Miss,
        Some((Some(json), _)) => match serde_json::from_str(&json) {
            Ok(lyrics) => Cached::Hit(Some(lyrics)),
            Err(_) => Cached::Miss,
        },
    })
}

pub fn put(conn: &Connection, key: &str, lyrics: Option<&Lyrics>, now: i64) -> anyhow::Result<()> {
    let json = lyrics.map(serde_json::to_string).transpose()?;
    conn.execute(
        "INSERT INTO lyrics_cache (key, lyrics, checked_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET lyrics = excluded.lyrics, checked_at = excluded.checked_at",
        params![key, json, now],
    )?;
    Ok(())
}

/// `lrclib:<artist>|<title>|<album>|<duration s>`, lowercased and trimmed.
pub fn lrclib_key(q: &Query) -> String {
    let part = |text: &str| {
        text.trim()
            .to_lowercase()
            .replace('\\', "\\\\")
            .replace('|', "\\|")
    };
    format!(
        "lrclib:{}|{}|{}|{}",
        part(&q.artist),
        part(&q.title),
        part(q.album.as_deref().unwrap_or("")),
        q.duration_secs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::LyricsSource;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../db/migrations/0010_lyrics.sql"))
            .unwrap();
        conn
    }

    fn sample() -> Lyrics {
        Lyrics::synced(
            LyricsSource::Lrclib,
            vec![(1_000, "First".into()), (2_500, "Second".into())],
        )
        .unwrap()
    }

    #[test]
    fn unknown_key_is_a_miss() {
        assert_eq!(get(&conn(), "lrclib:x", 100).unwrap(), Cached::Miss);
    }

    #[test]
    fn found_lyrics_round_trip_and_never_expire() {
        let conn = conn();
        let lyrics = sample();
        put(&conn, "k", Some(&lyrics), 100).unwrap();
        assert_eq!(
            get(&conn, "k", 100 + 10 * MISS_TTL_SECS).unwrap(),
            Cached::Hit(Some(lyrics))
        );
    }

    #[test]
    fn instrumental_round_trips() {
        let conn = conn();
        let lyrics = Lyrics::instrumental(LyricsSource::Lrclib);
        put(&conn, "k", Some(&lyrics), 0).unwrap();
        assert_eq!(get(&conn, "k", 5).unwrap(), Cached::Hit(Some(lyrics)));
    }

    #[test]
    fn a_miss_expires_after_seven_days() {
        let conn = conn();
        put(&conn, "k", None, 1_000).unwrap();
        assert_eq!(
            get(&conn, "k", 1_000 + MISS_TTL_SECS).unwrap(),
            Cached::Hit(None)
        );
        assert_eq!(
            get(&conn, "k", 1_000 + MISS_TTL_SECS + 1).unwrap(),
            Cached::Miss
        );
    }

    #[test]
    fn put_replaces_an_older_entry() {
        let conn = conn();
        put(&conn, "k", None, 10).unwrap();
        put(&conn, "k", Some(&sample()), 20).unwrap();
        assert_eq!(get(&conn, "k", 21).unwrap(), Cached::Hit(Some(sample())));
        put(&conn, "k", None, 30).unwrap();
        assert_eq!(get(&conn, "k", 31).unwrap(), Cached::Hit(None));
    }

    #[test]
    fn unreadable_json_is_a_miss() {
        let conn = conn();
        conn.execute(
            "INSERT INTO lyrics_cache (key, lyrics, checked_at) VALUES ('k', '{nope', 1)",
            [],
        )
        .unwrap();
        assert_eq!(get(&conn, "k", 2).unwrap(), Cached::Miss);
    }

    #[test]
    fn lrclib_key_is_lowercase_and_trimmed() {
        let q = Query {
            artist: " Test Artist ".into(),
            title: "Song One".into(),
            album: Some("Album ".into()),
            duration_ms: 201_600,
        };
        assert_eq!(lrclib_key(&q), "lrclib:test artist|song one|album|202");
        let q = Query {
            album: None,
            duration_ms: 0,
            ..q
        };
        assert_eq!(lrclib_key(&q), "lrclib:test artist|song one||0");
    }

    #[test]
    fn lrclib_key_is_unambiguous_with_separators_in_fields() {
        let q = |artist: &str, title: &str| Query {
            artist: artist.into(),
            title: title.into(),
            album: None,
            duration_ms: 1_000,
        };
        assert_ne!(lrclib_key(&q("A|B", "C")), lrclib_key(&q("A", "B|C")));
        assert_ne!(lrclib_key(&q("A\\", "B")), lrclib_key(&q("A", "\\B")));
    }
}
