-- Home pagination reserves `/page/N/` as product routes in this migration.
-- Earlier versions allowed `page` as a content slug, tag slug, or historical
-- redirect; those move to deterministic, collision-free slugs exactly as
-- migration 0007 did for `search`, with revisions and navigation carried
-- along and a historical redirect left behind for the old address.

-- `/page/` becomes a product route in this migration. Earlier versions
-- allowed that word as a content slug, tag slug, or historical redirect. Move
-- active values to deterministic, collision-free slugs before the Rust domain
-- starts rejecting new uses of the reserved route. Revisions and internal
-- navigation move with the content so an upgrade never makes stored JSON
-- unreadable or leaves an owner-facing link pointed at the pagination route.
CREATE TEMP TABLE legacy_page_content_slugs (
    content_id INTEGER PRIMARY KEY,
    new_slug TEXT NOT NULL UNIQUE
) STRICT;

WITH attempts(attempt) AS (
    SELECT 0
    UNION ALL
    SELECT row_number() OVER (ORDER BY identity)
    FROM (
        SELECT printf('content:%020d', id) AS identity FROM contents
        UNION ALL
        SELECT 'redirect:' || old_slug AS identity FROM redirects
    )
),
candidates(content_id, attempt, candidate) AS (
    SELECT contents.id,
           attempts.attempt,
           CASE attempts.attempt
               WHEN 0 THEN 'page-content-' || contents.id
               ELSE 'page-content-' || contents.id || '-' || attempts.attempt
           END
    FROM contents CROSS JOIN attempts
    WHERE contents.slug = 'page' COLLATE NOCASE
)
-- `contents.slug` has been UNIQUE COLLATE NOCASE since migration 0001, so a
-- valid legacy database can contain at most one case-insensitive match.
INSERT INTO legacy_page_content_slugs (content_id, new_slug)
SELECT content_id, candidate
FROM candidates
WHERE NOT EXISTS (SELECT 1 FROM contents WHERE slug = candidate)
  AND NOT EXISTS (SELECT 1 FROM redirects WHERE old_slug = candidate)
ORDER BY attempt
LIMIT 1;

CREATE TEMP TABLE legacy_page_tag_slugs (
    tag_id INTEGER PRIMARY KEY,
    new_slug TEXT NOT NULL UNIQUE
) STRICT;

WITH attempts(attempt) AS (
    SELECT 0
    UNION ALL
    SELECT row_number() OVER (ORDER BY id) FROM tags
),
candidates(tag_id, attempt, candidate) AS (
    SELECT tags.id,
           attempts.attempt,
           CASE attempts.attempt
               WHEN 0 THEN 'page-tag-' || tags.id
               ELSE 'page-tag-' || tags.id || '-' || attempts.attempt
           END
    FROM tags CROSS JOIN attempts
    WHERE tags.slug = 'page' COLLATE NOCASE
)
-- The same schema invariant holds for `tags.slug`; LIMIT 1 chooses the first
-- free candidate for that sole possible row, not one row from a larger set.
INSERT INTO legacy_page_tag_slugs (tag_id, new_slug)
SELECT tag_id, candidate
FROM candidates
WHERE NOT EXISTS (SELECT 1 FROM tags WHERE slug = candidate)
ORDER BY attempt
LIMIT 1;

-- A historical redirect can exist only when no active content owns `page`.
UPDATE navigation
SET destination = '/' || (
        SELECT contents.slug
        FROM redirects JOIN contents ON contents.id = redirects.content_id
        WHERE redirects.old_slug = 'page' COLLATE NOCASE
    ) || CASE destination WHEN '/page/' THEN '/' ELSE '' END
WHERE is_external = 0
  AND destination IN ('/page', '/page/')
  AND EXISTS (SELECT 1 FROM redirects WHERE old_slug = 'page' COLLATE NOCASE);

UPDATE navigation
SET destination = '/' || (
        SELECT new_slug FROM legacy_page_content_slugs LIMIT 1
    ) || CASE destination WHEN '/page/' THEN '/' ELSE '' END
WHERE is_external = 0
  AND destination IN ('/page', '/page/')
  AND EXISTS (SELECT 1 FROM legacy_page_content_slugs);

UPDATE revisions
SET snapshot_json = json_set(
        snapshot_json,
        '$.slug', COALESCE(
            (
                SELECT new_slug
                FROM legacy_page_content_slugs
                WHERE content_id = revisions.content_id
            ),
            (SELECT slug FROM contents WHERE id = revisions.content_id)
        )
    )
WHERE json_extract(snapshot_json, '$.slug') = 'page' COLLATE NOCASE;

UPDATE revisions
SET snapshot_json = json_set(
        snapshot_json,
        '$.tags', json((
            SELECT json_group_array(json_object(
                'name', json_extract(item.value, '$.name'),
                'slug', CASE
                    WHEN json_extract(item.value, '$.slug') = 'page' COLLATE NOCASE
                    THEN COALESCE(
                        (SELECT new_slug FROM legacy_page_tag_slugs LIMIT 1),
                        'page-tag-legacy'
                    )
                    ELSE json_extract(item.value, '$.slug')
                END
            ))
            FROM json_each(revisions.snapshot_json, '$.tags') AS item
        ))
    )
WHERE EXISTS (
    SELECT 1
    FROM json_each(revisions.snapshot_json, '$.tags') AS item
    WHERE json_extract(item.value, '$.slug') = 'page' COLLATE NOCASE
);

UPDATE contents
SET slug = (
    SELECT new_slug
    FROM legacy_page_content_slugs
    WHERE content_id = contents.id
)
WHERE id IN (SELECT content_id FROM legacy_page_content_slugs);

UPDATE tags
SET slug = (
    SELECT new_slug
    FROM legacy_page_tag_slugs
    WHERE tag_id = tags.id
)
WHERE id IN (SELECT tag_id FROM legacy_page_tag_slugs);

DELETE FROM redirects WHERE old_slug = 'page' COLLATE NOCASE;
DROP TABLE legacy_page_content_slugs;
DROP TABLE legacy_page_tag_slugs;

