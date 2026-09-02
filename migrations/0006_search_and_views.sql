-- Full-text search over public content, CJK-first. The trigram tokenizer
-- matches substrings of >= 3 characters in any script; shorter terms (the
-- two-character kanji compounds Japanese is made of) are served by LIKE
-- scans over the folded columns. Rows are keyed by content id (rowid) and
-- written from Rust, where NFKC normalization and kana folding happen.
CREATE VIRTUAL TABLE search_index USING fts5(
    title_fold,
    body_fold,
    title UNINDEXED,
    body UNINDEXED,
    tokenize = 'trigram'
);

-- Server-side view counter, owner-facing only. One row per content.
CREATE TABLE content_views (
    content_id INTEGER PRIMARY KEY REFERENCES contents(id) ON DELETE CASCADE,
    view_count INTEGER NOT NULL DEFAULT 0 CHECK (view_count >= 0)
) STRICT;
