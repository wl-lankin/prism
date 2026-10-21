//! Two-way stars. A heart on a Navidrome item writes the local favorite at once (source
//! `navidrome`) and queues the intent in `navidrome_star_queue`, latest intent per item. The
//! queue lives in the database, so it survives a restart, and [`flush`] sends it oldest first.
//! [`import`] makes the server's stars the truth for every item without a pending intent; a
//! pending intent wins until it is sent.

use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension};

use super::client::{NavidromeError, StarTarget, Starred2, SubsonicClient};
use crate::db::Db;
use crate::library::favorites::{set_with_source, FavKind};
use crate::library::model::Source;
use crate::library::scan::lock;

/// The id prefix of every Navidrome item.
pub const ID_PREFIX: &str = "navidrome:";

/// The server target of a Navidrome item, `None` for any other id.
pub fn target(kind: FavKind, id: &str) -> Option<StarTarget> {
    let raw = id.strip_prefix(ID_PREFIX).filter(|raw| !raw.is_empty())?;
    let raw = raw.to_owned();
    Some(match kind {
        FavKind::Track => StarTarget::Song(raw),
        FavKind::Album => StarTarget::Album(raw),
        FavKind::Artist => StarTarget::Artist(raw),
    })
}

/// Sets the local favorite of a Navidrome item (source `navidrome`) and queues the intent for
/// the server; a queued intent for the same item is replaced. Returns whether the local
/// favorite changed. Fails for ids that are not Navidrome ids.
pub fn set_favorite(
    conn: &Connection,
    kind: FavKind,
    id: &str,
    on: bool,
    now_ms: i64,
) -> anyhow::Result<bool> {
    if target(kind, id).is_none() {
        anyhow::bail!("\"{id}\" is not a Navidrome item");
    }
    let tx = conn.unchecked_transaction()?;
    let changed = set_with_source(&tx, kind, id, on, Source::Navidrome, now_ms)?;
    tx.execute(
        "INSERT INTO navidrome_star_queue (kind, item_id, star, queued_at, attempts)
         VALUES (?1, ?2, ?3, ?4, 0)
         ON CONFLICT(kind, item_id) DO UPDATE SET
             star = excluded.star, queued_at = excluded.queued_at, attempts = 0",
        params![kind.as_str(), id, on, now_ms],
    )?;
    tx.commit()?;
    Ok(changed)
}

/// One queued intent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingStar {
    pub kind: FavKind,
    pub item_id: String,
    pub star: bool,
    pub attempts: u32,
    /// When the intent was queued; a flush clears the row only while it is still this one.
    pub queued_at: i64,
}

/// Every queued intent, oldest first.
pub fn pending(conn: &Connection) -> anyhow::Result<Vec<PendingStar>> {
    let mut stmt = conn.prepare(
        "SELECT kind, item_id, star, attempts, queued_at FROM navidrome_star_queue
         ORDER BY queued_at, rowid",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, bool>(2)?,
            row.get::<_, u32>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (kind, item_id, star, attempts, queued_at) = row?;
        // Only the three kinds are ever written.
        let Some(kind) = FavKind::from_db(&kind) else {
            continue;
        };
        out.push(PendingStar {
            kind,
            item_id,
            star,
            attempts,
            queued_at,
        });
    }
    Ok(out)
}

/// What one flush did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlushOutcome {
    /// Intents the server took.
    pub sent: u32,
    /// Intents dropped: the item is gone from the server, or the server refused it.
    pub dropped: u32,
    /// The server could not be reached; the rest of the queue stays for a retry.
    pub offline: bool,
}

/// Sends the queue, one request per item, oldest first.
pub fn flush(db: &Db, client: &SubsonicClient) -> FlushOutcome {
    flush_cancellable(db, client, &|| false)
}

/// Errors that say nothing about the item: the account or the connection is wrong. The flush
/// stops and keeps the queue.
fn account_level(err: &NavidromeError) -> bool {
    matches!(
        err,
        NavidromeError::InvalidUrl(_)
            | NavidromeError::WrongCredentials
            | NavidromeError::TokenAuthUnsupported
            | NavidromeError::Tls(_)
            | NavidromeError::NotSubsonic(_)
    )
}

/// Deletes the queue row of `item` while it still holds the intent that was sent.
fn clear_sent(conn: &Connection, item: &PendingStar) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "DELETE FROM navidrome_star_queue
         WHERE kind = ?1 AND item_id = ?2 AND star = ?3 AND queued_at = ?4",
        params![item.kind.as_str(), item.item_id, item.star, item.queued_at],
    )? > 0)
}

/// [`flush`] that stops once `cancelled` says so (the account changed or the app quits).
/// `cancelled` is checked under the database lock before every write, and network calls run
/// outside that lock.
pub fn flush_cancellable(
    db: &Db,
    client: &SubsonicClient,
    cancelled: &dyn Fn() -> bool,
) -> FlushOutcome {
    let mut outcome = FlushOutcome::default();
    let queue = {
        let Ok(conn) = lock(db) else {
            return outcome;
        };
        if cancelled() {
            return outcome;
        }
        match pending(&conn) {
            Ok(queue) => queue,
            Err(err) => {
                eprintln!("reading the Navidrome star queue failed: {err:#}");
                return outcome;
            }
        }
    };
    for item in queue {
        if cancelled() {
            break;
        }
        let result = match target(item.kind, &item.item_id) {
            Some(target) => client.star(&target, item.star),
            None => Err(NavidromeError::NotFound),
        };
        let Ok(conn) = lock(db) else {
            break;
        };
        if cancelled() {
            break;
        }
        let written = match &result {
            Ok(()) => clear_sent(&conn, &item).map(|_| outcome.sent += 1),
            Err(err) if err.is_offline() => {
                outcome.offline = true;
                conn.execute(
                    "UPDATE navidrome_star_queue SET attempts = attempts + 1
                     WHERE kind = ?1 AND item_id = ?2 AND queued_at = ?3",
                    params![item.kind.as_str(), item.item_id, item.queued_at],
                )
                .map(|_| ())
            }
            Err(err) if account_level(err) => Ok(()),
            Err(err) => clear_sent(&conn, &item).and_then(|cleared| {
                outcome.dropped += 1;
                // The item is gone: a star of it cannot be kept. Only while no newer intent
                // replaced the one that was sent.
                if cleared && item.star && *err == NavidromeError::NotFound {
                    conn.execute(
                        "DELETE FROM favorites WHERE kind = ?1 AND item_id = ?2",
                        params![item.kind.as_str(), item.item_id],
                    )?;
                }
                Ok(())
            }),
        };
        if let Err(err) = written {
            eprintln!("updating the Navidrome star queue failed: {err}");
            break;
        }
        if let Err(err) = &result {
            if err.is_offline() || account_level(err) {
                break;
            }
            eprintln!("Navidrome dropped a star change: {}", err.code());
        }
    }
    outcome
}

/// The starred items of the server as `(kind, item id, starred timestamp)`.
fn server_stars(starred: &Starred2) -> Vec<(FavKind, String, Option<&str>)> {
    let songs = starred
        .song
        .iter()
        .map(|s| (FavKind::Track, s.id.as_str(), s.starred.as_deref()));
    let albums = starred
        .album
        .iter()
        .map(|a| (FavKind::Album, a.id.as_str(), a.starred.as_deref()));
    let artists = starred
        .artist
        .iter()
        .map(|a| (FavKind::Artist, a.id.as_str(), a.starred.as_deref()));
    songs
        .chain(albums)
        .chain(artists)
        .filter(|(_, id, _)| !id.is_empty())
        .map(|(kind, id, at)| (kind, format!("{ID_PREFIX}{id}"), at))
        .collect()
}

/// Makes the server's stars the local Navidrome favorites, for every item without a pending
/// intent: adds server stars missing locally (`added_at` from the server's timestamp, else
/// `now_ms`) and removes local Navidrome favorites the server no longer stars. Returns whether
/// anything changed.
pub fn import(conn: &mut Connection, starred: &Starred2, now_ms: i64) -> anyhow::Result<bool> {
    let tx = conn.transaction()?;
    let queued: HashSet<(String, String)> = {
        let mut stmt = tx.prepare("SELECT kind, item_id FROM navidrome_star_queue")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let local: HashSet<(String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT kind, item_id FROM favorites
             WHERE source = 'navidrome' OR item_id GLOB 'navidrome:*'",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let mut on_server: HashSet<(String, String)> = HashSet::new();
    let mut changed = false;
    for (kind, id, starred_at) in server_stars(starred) {
        let key = (kind.as_str().to_owned(), id);
        if !queued.contains(&key) && !local.contains(&key) {
            let added_at = match starred_at {
                Some(at) => tx
                    .query_row(
                        "SELECT CAST(ROUND((julianday(?1) - 2440587.5) * 86400000.0) AS INTEGER)",
                        [at],
                        |row| row.get::<_, Option<i64>>(0),
                    )
                    .optional()?
                    .flatten(),
                None => None,
            }
            .unwrap_or(now_ms);
            changed |= set_with_source(&tx, kind, &key.1, true, Source::Navidrome, added_at)?;
        }
        on_server.insert(key);
    }
    for key in &local {
        if on_server.contains(key) || queued.contains(key) {
            continue;
        }
        changed |= tx.execute(
            "DELETE FROM favorites WHERE kind = ?1 AND item_id = ?2",
            params![key.0, key.1],
        )? > 0;
    }
    tx.commit()?;
    Ok(changed)
}

#[cfg(test)]
mod tests;
