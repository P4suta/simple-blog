CREATE TABLE content_likes (
    content_id INTEGER PRIMARY KEY REFERENCES contents(id) ON DELETE CASCADE,
    like_count INTEGER NOT NULL DEFAULT 0 CHECK (like_count >= 0)
) STRICT;
