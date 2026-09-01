-- Static publication is driven by a monotonic revision. The next scheduled
-- boundary is durable so every host adapter can resume scheduling after a
-- restart without scanning in-memory state.
CREATE TABLE publication_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    public_revision INTEGER NOT NULL DEFAULT 0 CHECK (public_revision >= 0),
    next_publish_at TEXT,
    updated_at TEXT NOT NULL
) STRICT;

INSERT INTO publication_state (singleton, public_revision, next_publish_at, updated_at)
SELECT 1, 0, MIN(publish_at), '1970-01-01T00:00:00Z'
FROM contents
WHERE status = 'public' AND publish_at > CURRENT_TIMESTAMP;
