PRAGMA foreign_keys = ON;

CREATE TABLE media (
    id TEXT PRIMARY KEY,
    original_name TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    extension TEXT NOT NULL,
    width INTEGER NOT NULL CHECK (width > 0),
    height INTEGER NOT NULL CHECK (height > 0),
    byte_size INTEGER NOT NULL CHECK (byte_size > 0),
    alt_text TEXT NOT NULL DEFAULT '',
    caption TEXT NOT NULL DEFAULT '',
    animated INTEGER NOT NULL DEFAULT 0 CHECK (animated IN (0, 1)),
    created_at TEXT NOT NULL
) STRICT;

CREATE TABLE contents (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('post', 'page')),
    title TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 200),
    slug TEXT NOT NULL UNIQUE COLLATE NOCASE,
    summary TEXT NOT NULL DEFAULT '',
    body_markdown TEXT NOT NULL,
    body_html TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('draft', 'public')),
    publish_at TEXT,
    cover_media_id TEXT REFERENCES media(id) ON DELETE SET NULL,
    seo_title TEXT,
    seo_description TEXT,
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK ((status = 'draft' AND publish_at IS NULL) OR (status = 'public' AND publish_at IS NOT NULL))
) STRICT;

CREATE INDEX contents_publication_idx ON contents(status, publish_at DESC, id DESC);

CREATE TABLE tags (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    slug TEXT NOT NULL UNIQUE COLLATE NOCASE
) STRICT;

CREATE TABLE content_tags (
    content_id INTEGER NOT NULL REFERENCES contents(id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    PRIMARY KEY (content_id, tag_id),
    UNIQUE (content_id, position)
) STRICT;

CREATE TABLE revisions (
    id INTEGER PRIMARY KEY,
    content_id INTEGER NOT NULL REFERENCES contents(id) ON DELETE CASCADE,
    intent TEXT NOT NULL CHECK (intent IN ('autosave', 'explicit')),
    snapshot_json TEXT NOT NULL CHECK (json_valid(snapshot_json)),
    created_at TEXT NOT NULL
) STRICT;

CREATE INDEX revisions_timeline_idx ON revisions(content_id, created_at DESC, id DESC);

CREATE TABLE redirects (
    old_slug TEXT PRIMARY KEY COLLATE NOCASE,
    content_id INTEGER NOT NULL REFERENCES contents(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL
) STRICT;

CREATE TRIGGER contents_slug_not_historical_insert
BEFORE INSERT ON contents
WHEN EXISTS (SELECT 1 FROM redirects WHERE old_slug = NEW.slug)
BEGIN
    SELECT RAISE(ABORT, 'slug is historical');
END;

CREATE TRIGGER contents_slug_not_historical_update
BEFORE UPDATE OF slug ON contents
WHEN EXISTS (SELECT 1 FROM redirects WHERE old_slug = NEW.slug AND content_id != NEW.id)
BEGIN
    SELECT RAISE(ABORT, 'slug is historical');
END;

CREATE TRIGGER redirects_slug_not_active
BEFORE INSERT ON redirects
WHEN EXISTS (SELECT 1 FROM contents WHERE slug = NEW.old_slug)
BEGIN
    SELECT RAISE(ABORT, 'slug is active');
END;

CREATE TABLE navigation (
    id INTEGER PRIMARY KEY,
    label TEXT NOT NULL,
    destination TEXT NOT NULL,
    is_external INTEGER NOT NULL CHECK (is_external IN (0, 1)),
    position INTEGER NOT NULL UNIQUE CHECK (position >= 0)
) STRICT;

CREATE TABLE site_settings (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    site_title TEXT NOT NULL,
    site_description TEXT NOT NULL,
    locale TEXT NOT NULL CHECK (locale IN ('ja', 'en')),
    logo_media_id TEXT REFERENCES media(id) ON DELETE SET NULL,
    favicon_media_id TEXT REFERENCES media(id) ON DELETE SET NULL,
    accent_color TEXT NOT NULL,
    font_preset TEXT NOT NULL CHECK (font_preset IN ('sans', 'serif')),
    content_width INTEGER NOT NULL CHECK (content_width BETWEEN 560 AND 960),
    color_scheme TEXT NOT NULL CHECK (color_scheme IN ('system', 'light', 'dark')),
    custom_css TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL
) STRICT;

INSERT INTO site_settings (
    singleton, site_title, site_description, locale, accent_color,
    font_preset, content_width, color_scheme, updated_at
) VALUES (1, 'Simple Blog', '', 'ja', '#3867d6', 'serif', 720, 'system', '1970-01-01T00:00:00Z');

CREATE TABLE owner (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    user_handle BLOB NOT NULL UNIQUE,
    created_at TEXT NOT NULL
) STRICT;

CREATE TABLE passkeys (
    credential_id BLOB PRIMARY KEY,
    name TEXT NOT NULL,
    passkey_json TEXT NOT NULL CHECK (json_valid(passkey_json)),
    created_at TEXT NOT NULL,
    last_used_at TEXT
) STRICT;

CREATE TABLE setup_tokens (
    token_hash BLOB PRIMARY KEY,
    purpose TEXT NOT NULL CHECK (purpose IN ('setup', 'recover')),
    expires_at TEXT NOT NULL,
    consumed_at TEXT
) STRICT;

CREATE TABLE recovery_codes (
    code_hash BLOB PRIMARY KEY,
    consumed_at TEXT,
    created_at TEXT NOT NULL
) STRICT;

CREATE TABLE sessions (
    token_hash BLOB PRIMARY KEY,
    csrf_token_hash BLOB NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    reauthenticated_at TEXT NOT NULL
) STRICT;

CREATE INDEX sessions_expiration_idx ON sessions(expires_at);
