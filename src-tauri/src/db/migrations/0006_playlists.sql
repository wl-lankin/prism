-- Playlists. Entries are ordered by `position` (0..n, renumbered on every change) and may repeat
-- a track. track_id has no foreign key to tracks: an entry whose track left the index (folder
-- offline, file removed) is kept and plays again when the track returns. Rekeys rewrite it
-- through TRACK_ID_REFS. Deleting a playlist deletes its entries.
CREATE TABLE playlists (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE playlist_tracks (
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    track_id TEXT NOT NULL,
    PRIMARY KEY (playlist_id, position)
);

CREATE INDEX idx_playlist_tracks_track ON playlist_tracks (track_id);
