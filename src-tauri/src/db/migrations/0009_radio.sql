-- Radio. A station plays as a track with source = 'radio' and id `radio:<stationuuid>`, but it
-- never gets a row in tracks / albums: this table is its only home. Only stations the user
-- saved (a favorite of kind 'station') or played are stored; a row is kept while it is a
-- favorite or among the most recently played, the rest is pruned. No foreign keys: a
-- favorite whose station row is gone is kept and listed again when the station returns.
-- `tags` is the cleaned tag list joined by commas; `cover_id` is the `rb-` id of the logo.
CREATE TABLE radio_stations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    url TEXT NOT NULL,
    url_resolved TEXT,
    homepage TEXT,
    favicon TEXT,
    cover_id TEXT,
    country_code TEXT,
    region TEXT,
    language TEXT,
    tags TEXT NOT NULL DEFAULT '',
    codec TEXT,
    bitrate_kbps INTEGER,
    hls INTEGER NOT NULL DEFAULT 0,
    last_check_ok INTEGER NOT NULL DEFAULT 1,
    saved_at INTEGER NOT NULL,
    refreshed_at INTEGER,
    last_played_at INTEGER,
    last_click_at INTEGER
);

CREATE INDEX idx_radio_stations_played ON radio_stations (last_played_at);

-- Where a station logo comes from, by cover id. Search and near results write theirs here
-- too, so their tiles can load without a stored station. At least one of `url` (the
-- station's favicon) and `homepage` (the fallback to read an icon from) is set.
-- `missing_until` (ms) silences a logo that did not load until then.
CREATE TABLE radio_favicons (
    cover_id TEXT PRIMARY KEY,
    url TEXT,
    homepage TEXT,
    missing_until INTEGER
);

-- Radio bookkeeping: the directory mirror list, the detected place, the near-you cache.
CREATE TABLE radio_state (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
