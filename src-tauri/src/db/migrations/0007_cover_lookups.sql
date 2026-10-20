-- Online cover lookups (MusicBrainz + Cover Art Archive). One row per album that was looked up:
-- `found` carries the cover id in the cover store, `notfound` is retried after 30 days. Network
-- errors are not recorded. No foreign key to albums: rows survive an album being rebuilt.
CREATE TABLE cover_lookups (
    album_id TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    cover_id TEXT,
    checked_at INTEGER NOT NULL
);
