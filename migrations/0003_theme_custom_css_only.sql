-- Design settings collapse into a single custom CSS document, the locale set
-- gains Chinese, and the default language becomes English. The former default
-- theme becomes ordinary, user-editable custom CSS: rows that never customized
-- their CSS are seeded with it once, here. An empty custom_css afterwards is a
-- deliberate "no styles" choice and is never re-seeded. Rows whose settings
-- were never saved (updated_at still the epoch sentinel) switch to the new
-- English default; a saved locale choice is preserved.
CREATE TABLE site_settings_new (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    site_title TEXT NOT NULL,
    site_description TEXT NOT NULL,
    locale TEXT NOT NULL CHECK (locale IN ('en', 'ja', 'zh')),
    logo_media_id TEXT REFERENCES media(id) ON DELETE SET NULL,
    favicon_media_id TEXT REFERENCES media(id) ON DELETE SET NULL,
    custom_css TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL
) STRICT;

INSERT INTO site_settings_new (
    singleton, site_title, site_description, locale,
    logo_media_id, favicon_media_id, custom_css, updated_at
)
SELECT singleton, site_title, site_description, locale,
       logo_media_id, favicon_media_id, custom_css, updated_at
FROM site_settings;

DROP TABLE site_settings;
ALTER TABLE site_settings_new RENAME TO site_settings;

UPDATE site_settings SET locale = 'en' WHERE updated_at = '1970-01-01T00:00:00Z';

UPDATE site_settings SET custom_css = '/* Simple Blog default theme.
   Ordinary custom CSS: edit it, replace it, or empty it in Settings.
   It concerns itself with layout only — measure, rhythm, and alignment.
   Colors, backgrounds, and decoration are left entirely to the browser. */

:root {
  color-scheme: light dark;
}

/* ---- Measure and typesetting ---------------------------------------- */

body {
  max-width: 38rem;
  margin-inline: auto;
  padding: 1.5rem 1rem 4rem;
  font-family: system-ui, sans-serif;
  line-height: 1.9;
  font-kerning: normal;
  line-break: strict;
  overflow-wrap: anywhere;
  hanging-punctuation: allow-end;
}

p {
  text-wrap: pretty;
}

h1,
h2,
h3,
h4 {
  line-height: 1.4;
  text-wrap: balance;
  margin-block: 2em 0.6em;
}

h1 {
  font-size: 1.5rem;
}

h2 {
  font-size: 1.25rem;
}

h3 {
  font-size: 1.05rem;
}

h4 {
  font-size: 1rem;
}

/* One shared block rhythm for flowing content */
p,
ul,
ol,
dl,
table,
pre,
blockquote,
figure {
  margin-block: 1em;
}

ul,
ol {
  padding-inline-start: 1.6em;
}

li {
  margin-block: 0.2em;
}

dd {
  margin-inline-start: 1.6em;
}

blockquote {
  margin-inline: 1.6em 0;
}

hr {
  margin-block: 2.5em;
}

sub,
sup {
  line-height: 1;
}

/* Dates and other figures align in columns */
time {
  font-variant-numeric: tabular-nums;
}

/* ---- Media and preformatted blocks ---------------------------------- */

img,
video {
  max-width: 100%;
  height: auto;
}

figure {
  margin-inline: 0;
}

figcaption {
  font-size: 0.85em;
  margin-block-start: 0.4em;
}

pre {
  overflow-x: auto;
  line-height: 1.6;
  tab-size: 4;
}

/* ---- Tables ---------------------------------------------------------- */

table {
  border-collapse: collapse;
}

th,
td {
  text-align: left;
  vertical-align: baseline;
  padding: 0.15em 1.5em 0.15em 0;
}

th:last-child,
td:last-child {
  padding-inline-end: 0;
}

/* Wide tables inside an article scroll instead of breaking the measure */
.prose table {
  display: block;
  overflow-x: auto;
}

/* ---- Inline link lists (header nav, footer nav, tags) ---------------- */

.site-header nav ul,
.site-footer nav ul,
.tags {
  display: flex;
  flex-wrap: wrap;
  column-gap: 1em;
  row-gap: 0.2em;
  list-style: none;
  margin: 0;
  padding: 0;
}

.site-header nav li,
.site-footer nav li,
.tags li {
  margin: 0;
}

/* ---- Header and footer: one baseline-aligned line each ---------------- */

.site-header {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  column-gap: 1.5em;
  row-gap: 0.3em;
  margin-block-end: 3em;
}

.site-header .brand {
  font-weight: 700;
}

.site-logo {
  height: 1.2em;
  vertical-align: -0.2em;
  margin-inline-end: 0.4em;
}

.site-footer {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  column-gap: 1.5em;
  row-gap: 0.3em;
  margin-block-start: 5em;
}

/* ---- Home ------------------------------------------------------------ */

.hero {
  margin-block-end: 2.5em;
}

.hero h1 {
  margin-block: 0 0.3em;
}

.hero p {
  margin-block: 0;
}

/* Each entry is a grid: the date forms an aligned column, everything
   else hangs off the title column. */
.post-list article {
  display: grid;
  grid-template-columns: max-content 1fr;
  column-gap: 1.2em;
  align-items: baseline;
  margin-block: 1.4em;
}

.post-list time {
  grid-column: 1;
}

.post-list h2 {
  grid-column: 2;
  margin: 0;
  font-size: 1em;
}

.post-list p,
.post-list .tags {
  grid-column: 2;
  margin-block: 0.2em 0;
}

@media (max-width: 30rem) {
  .post-list article {
    grid-template-columns: 1fr;
  }

  .post-list h2,
  .post-list p,
  .post-list .tags {
    grid-column: 1;
  }
}

/* ---- Archive ---------------------------------------------------------- */

.page-header {
  margin-block-end: 1.5em;
}

.page-header h1 {
  margin-block: 0;
}

.archive-list td {
  padding-block: 0.25em;
}

/* ---- Article ---------------------------------------------------------- */

.article-header {
  margin-block-end: 2em;
}

.article-header time {
  font-size: 0.85em;
}

.article-header h1 {
  margin-block: 0.2em 0.4em;
}

.article-header .dek {
  margin-block: 0;
}

.cover {
  margin-block: 2em;
}

.prose :target {
  scroll-margin-block-start: 2em;
}

/* ---- Accessibility ---------------------------------------------------- */

.skip-link {
  position: absolute;
  left: -9999px;
}

.skip-link:focus {
  position: static;
}
' WHERE custom_css = '';
