-- Every save of the site's settings and navigation is kept, the way every
-- save of a piece is, so a writer can bring back the settings from before a
-- change; the restore is itself a new revision. The first save also records
-- the state it replaced, so nothing is ever lost to the first edit. Only the
-- newest fifty states are kept, and a save that changes nothing adds none.
CREATE TABLE site_settings_revisions (
    id INTEGER PRIMARY KEY,
    settings_json TEXT NOT NULL,
    navigation_json TEXT NOT NULL,
    created_at TEXT NOT NULL
) STRICT;
