CREATE TABLE media_variants (
    media_id TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    width INTEGER NOT NULL CHECK (width > 0),
    height INTEGER NOT NULL CHECK (height > 0),
    byte_size INTEGER NOT NULL CHECK (byte_size > 0),
    filename TEXT NOT NULL UNIQUE,
    PRIMARY KEY (media_id, width)
) STRICT;
