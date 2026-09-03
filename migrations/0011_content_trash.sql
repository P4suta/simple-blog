-- Content can be moved to the trash and restored. A trashed row keeps its
-- slug reserved and its media referenced, but leaves every public query
-- (snapshot, lists, neighbours, search, likes, redirects, publication clock)
-- until it is restored or deleted permanently.
ALTER TABLE contents ADD COLUMN deleted_at TEXT;

CREATE INDEX contents_trash_idx ON contents(deleted_at) WHERE deleted_at IS NOT NULL;
