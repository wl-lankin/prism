-- Navidrome. Server rows live in tracks / albums with source = 'navidrome' and ids
-- `navidrome:<subsonic id>`. A Navidrome album names its server artist in artist_id; local
-- albums leave it NULL and are grouped by name.
ALTER TABLE albums ADD COLUMN artist_id TEXT;

CREATE INDEX idx_albums_artist_id ON albums (artist_id);

-- idx_tracks_order with the source in front, so a source view pages the same way.
CREATE INDEX idx_tracks_source_order ON tracks (
    source,
    artist COLLATE NOCASE,
    album COLLATE NOCASE,
    disc_no IS NULL,
    disc_no,
    track_no IS NULL,
    track_no,
    title COLLATE NOCASE,
    id
);

-- The FTS row only changes with the indexed columns. A sync rewrites every Navidrome row;
-- rewriting the FTS entries for every play count or cover change is wasted work.
DROP TRIGGER tracks_fts_au;

CREATE TRIGGER tracks_fts_au AFTER UPDATE OF title, artist, album ON tracks BEGIN
    INSERT INTO tracks_fts (tracks_fts, rowid, title, artist, album)
    VALUES ('delete', old.row_id, old.title, old.artist, old.album);
    INSERT INTO tracks_fts (rowid, title, artist, album)
    VALUES (new.row_id, new.title, new.artist, new.album);
END;

-- Sync bookkeeping of the configured server (server key, lastModified, last full pass).
-- Wiped together with every Navidrome row when the server or user changes.
CREATE TABLE navidrome_state (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Star and unstar intents not yet confirmed by the server, latest intent per item. `kind` is
-- track, album or artist. item_id has no foreign key: an intent whose item left the index is
-- kept and still sent.
CREATE TABLE navidrome_star_queue (
    kind TEXT NOT NULL,
    item_id TEXT NOT NULL,
    star INTEGER NOT NULL,
    queued_at INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (kind, item_id)
);

-- Scrobble submissions the server did not take yet. track_id has no foreign key: a scrobble
-- whose track left the index is kept and still sent.
CREATE TABLE navidrome_scrobble_queue (
    id INTEGER PRIMARY KEY,
    track_id TEXT NOT NULL,
    played_at INTEGER NOT NULL
);

-- Playlists of the server, read-only in Prism. Entries are ordered by `position` and may
-- repeat a track. track_id has no foreign key to tracks: an entry whose track left the index
-- is kept and plays again when the track returns. Deleting a playlist deletes its entries.
CREATE TABLE remote_playlists (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    owner TEXT,
    changed TEXT,
    track_count INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL
);

CREATE TABLE remote_playlist_tracks (
    playlist_id TEXT NOT NULL REFERENCES remote_playlists(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    track_id TEXT NOT NULL,
    PRIMARY KEY (playlist_id, position)
);
