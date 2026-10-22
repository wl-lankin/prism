-- Local favorites. `kind` is track, album or artist; tracks are keyed by track id, albums by
-- album id, artists by name. item_id has no foreign key: a favorite whose item is not in the
-- index (folder offline, file removed) is kept and comes back with the item.
CREATE TABLE favorites (
    kind TEXT NOT NULL,
    item_id TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'local',
    added_at INTEGER NOT NULL,
    PRIMARY KEY (kind, item_id)
);
