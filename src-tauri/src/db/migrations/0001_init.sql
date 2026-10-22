CREATE TABLE folders (
    path TEXT PRIMARY KEY,
    online INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE tracks (
    row_id INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    source TEXT NOT NULL DEFAULT 'local',
    title TEXT NOT NULL,
    artist TEXT NOT NULL,
    album_artist TEXT NOT NULL,
    album TEXT NOT NULL,
    album_id TEXT NOT NULL,
    track_no INTEGER,
    disc_no INTEGER,
    duration_ms INTEGER NOT NULL,
    year INTEGER,
    genre TEXT,
    format TEXT NOT NULL,
    sample_rate INTEGER,
    bit_depth INTEGER,
    bitrate_kbps INTEGER,
    cover_id TEXT,
    path TEXT,
    replay_gain_track_db REAL,
    replay_gain_album_db REAL,
    replay_gain_track_peak REAL,
    replay_gain_album_peak REAL,
    folder TEXT,
    mtime INTEGER,
    size INTEGER
);

CREATE INDEX idx_tracks_album_id ON tracks (album_id);
CREATE INDEX idx_tracks_artist ON tracks (artist);
CREATE INDEX idx_tracks_album_artist ON tracks (album_artist);

CREATE TABLE albums (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL DEFAULT 'local',
    title TEXT NOT NULL,
    artist TEXT NOT NULL,
    year INTEGER,
    cover_id TEXT,
    track_count INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER NOT NULL DEFAULT 0
);

CREATE VIRTUAL TABLE tracks_fts USING fts5(
    title,
    artist,
    album,
    content = 'tracks',
    content_rowid = 'row_id'
);

CREATE TRIGGER tracks_fts_ai AFTER INSERT ON tracks BEGIN
    INSERT INTO tracks_fts (rowid, title, artist, album)
    VALUES (new.row_id, new.title, new.artist, new.album);
END;

CREATE TRIGGER tracks_fts_ad AFTER DELETE ON tracks BEGIN
    INSERT INTO tracks_fts (tracks_fts, rowid, title, artist, album)
    VALUES ('delete', old.row_id, old.title, old.artist, old.album);
END;

CREATE TRIGGER tracks_fts_au AFTER UPDATE ON tracks BEGIN
    INSERT INTO tracks_fts (tracks_fts, rowid, title, artist, album)
    VALUES ('delete', old.row_id, old.title, old.artist, old.album);
    INSERT INTO tracks_fts (rowid, title, artist, album)
    VALUES (new.row_id, new.title, new.artist, new.album);
END;

CREATE TABLE covers (
    id TEXT PRIMARY KEY,
    file TEXT NOT NULL
);
