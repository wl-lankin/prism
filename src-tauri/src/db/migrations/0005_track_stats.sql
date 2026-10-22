-- Play history per track. track_id has no foreign key: stats of a track that leaves the index
-- (folder offline, file removed) are kept and count again when it returns. Rekeys rewrite it
-- through TRACK_ID_REFS.
CREATE TABLE track_stats (
    track_id TEXT PRIMARY KEY,
    play_count INTEGER NOT NULL DEFAULT 0,
    last_played INTEGER
);

-- The genre Flow variant matches genres case-insensitively.
CREATE INDEX idx_tracks_genre ON tracks (genre COLLATE NOCASE);
