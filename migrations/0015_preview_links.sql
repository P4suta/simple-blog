-- A preview link is a bearer capability that lets whoever holds it read one
-- piece before publication. Only the hash is stored; links expire and can be
-- revoked, and they vanish with the piece. Like sessions, they are ephemeral
-- and never travel in a .simple-blog archive.
CREATE TABLE preview_links (
    token_hash BLOB PRIMARY KEY,
    content_id INTEGER NOT NULL REFERENCES contents(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
) STRICT;

CREATE INDEX preview_links_content_idx ON preview_links(content_id);
