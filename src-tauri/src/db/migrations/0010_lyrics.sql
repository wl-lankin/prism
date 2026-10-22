-- Lyrics found by the network sources (Navidrome, LRCLIB, YouTube Music), by lookup key such as
-- `lrclib:<artist>|<title>|<album>|<seconds>`, `navidrome:<song id>` or `youtube:<video id>`.
-- `lyrics` is the JSON of the model; NULL records "nothing found" and is retried after 7 days.
-- `checked_at` is unix seconds. Local files are never cached.
CREATE TABLE lyrics_cache (
    key TEXT PRIMARY KEY,
    lyrics TEXT,
    checked_at INTEGER NOT NULL
);
