CREATE INDEX idx_albums_artist ON albums (artist);
CREATE INDEX idx_tracks_folder ON tracks (folder);

-- Match TRACK_ORDER and ALBUM_ORDER of library/query.rs exactly so ranged list queries
-- walk the index instead of sorting the whole table.
CREATE INDEX idx_tracks_order ON tracks (
    artist COLLATE NOCASE,
    album COLLATE NOCASE,
    disc_no IS NULL,
    disc_no,
    track_no IS NULL,
    track_no,
    title COLLATE NOCASE,
    id
);

CREATE INDEX idx_albums_order ON albums (
    artist COLLATE NOCASE,
    year IS NULL,
    year,
    title COLLATE NOCASE,
    id
);
