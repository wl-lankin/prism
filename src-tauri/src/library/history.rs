//! Play history: how often and when each track was last heard (Flow weights by it).

use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};

use crate::db::Db;
use crate::player::{EventSink, PlayerEvent, DROPPED_PREFIX};

use super::scan::lock;

/// Counts one play of `track_id` at `at_ms` (Unix milliseconds).
pub fn record_play(conn: &Connection, track_id: &str, at_ms: i64) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO track_stats (track_id, play_count, last_played) VALUES (?1, 1, ?2)
         ON CONFLICT(track_id) DO UPDATE SET
            play_count = play_count + 1,
            last_played = excluded.last_played",
        params![track_id, at_ms],
    )?;
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// A player sink that records every `TrackChanged` of an indexed track. The database work runs
/// on its own thread so the player thread never waits for the library lock.
pub fn spawn_recorder(db: Arc<Db>) -> EventSink {
    spawn(db, now_ms).0
}

/// `spawn_recorder` with a clock, also handing out the thread (which ends once every clone
/// of the sink is dropped).
fn spawn(db: Arc<Db>, clock: fn() -> i64) -> (EventSink, JoinHandle<()>) {
    let (sender, receiver) = mpsc::channel::<(String, i64)>();
    let thread = std::thread::Builder::new()
        .name("prism-history".into())
        .spawn(move || {
            for (track_id, at_ms) in receiver {
                let result = lock(&db).and_then(|conn| record_play(&conn, &track_id, at_ms));
                if let Err(err) = result {
                    eprintln!("recording a play of {track_id} failed: {err:#}");
                }
            }
        })
        .expect("spawning the history thread");
    let sink: EventSink = Arc::new(move |event| {
        if let PlayerEvent::TrackChanged(track) = event {
            // Dropped files are not in the library; their stats would never be read.
            if track.id.starts_with(DROPPED_PREFIX) {
                return;
            }
            let _ = sender.send((track.id, clock()));
        }
    });
    (sink, thread)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::model::{Source, Track};

    fn track(id: &str) -> Track {
        Track {
            id: id.into(),
            source: Source::Local,
            title: "Song".into(),
            artist: "Artist".into(),
            album_artist: "Artist".into(),
            album: "Album".into(),
            album_id: "a1".into(),
            track_no: None,
            disc_no: None,
            duration_ms: 1000,
            year: None,
            genre: None,
            format: "flac".into(),
            sample_rate: None,
            bit_depth: None,
            bitrate_kbps: None,
            cover_id: None,
            path: None,
            replay_gain_track_db: None,
            replay_gain_album_db: None,
            replay_gain_track_peak: None,
            replay_gain_album_peak: None,
        }
    }

    fn stats(db: &Db, id: &str) -> Option<(i64, Option<i64>)> {
        use rusqlite::OptionalExtension;
        db.0.lock()
            .unwrap()
            .query_row(
                "SELECT play_count, last_played FROM track_stats WHERE track_id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .unwrap()
    }

    #[test]
    fn record_play_counts_and_stamps() {
        let db = Db::open_in_memory().unwrap();
        {
            let conn = db.0.lock().unwrap();
            record_play(&conn, "t1", 10).unwrap();
            record_play(&conn, "t1", 20).unwrap();
        }
        assert_eq!(stats(&db, "t1"), Some((2, Some(20))));
    }

    #[test]
    fn history_records_track_changes() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (sink, thread) = spawn(Arc::clone(&db), || 1_700_000_000_000);
        sink(PlayerEvent::TrackChanged(track("t1")));
        sink(PlayerEvent::TrackChanged(track("t1")));
        sink(PlayerEvent::TrackChanged(track(&format!(
            "{DROPPED_PREFIX}x"
        ))));
        drop(sink);
        thread.join().unwrap();
        assert_eq!(stats(&db, "t1"), Some((2, Some(1_700_000_000_000))));
        assert_eq!(
            db.0.lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM track_stats", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
