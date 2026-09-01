-- The default theme adopts the site-wide doctrine: no decoration, style
-- only to align and organize. Rounded corners and custom control styling
-- are gone (buttons render natively), all separators share one faded
-- currentColor hairline, and dimming is reserved for metadata. As always,
-- only stylesheets still equal to the previous default verbatim are updated.
UPDATE site_settings SET custom_css = '/* Simple Blog default theme.
   Ordinary custom CSS: edit it, replace it, or empty it in Settings.

   Doctrine: no decoration. Style exists only to align and organize —
   measure, rhythm, aligned columns, structural hairlines. Colors,
   backgrounds, and the look of controls are left entirely to the browser.
   Every visible stroke below is the same faded currentColor hairline; the
   only dimming is on metadata (dates, captions, appendices), never on prose. */

:root {
  color-scheme: light dark;
}

/* ---- Reader preferences ----------------------------------------------- */
/* Taste differs per person, so measure, text size, and scheme belong to the
   visitor: prefs.js stores each reader''s choice in their browser and mirrors
   it as data attributes on the html element, which these hooks pick up.
   Without JavaScript the defaults simply hold. */

:root[data-measure="narrow"] body {
  max-width: 32rem;
}

:root[data-measure="wide"] body {
  max-width: 46rem;
}

:root[data-text="small"] body {
  font-size: 0.875rem;
}

:root[data-text="large"] body {
  font-size: 1.125rem;
}

:root[data-scheme="light"] {
  color-scheme: only light;
}

:root[data-scheme="dark"] {
  color-scheme: only dark;
}

.prefs {
  margin-block-start: 2.5em;
  font-size: 0.85em;
}

.prefs summary {
  cursor: pointer;
  opacity: 0.6;
}

/* Each row is legend + three options; fixed minimum widths keep the option
   columns vertically aligned across all three rows. */
.prefs fieldset {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  border: 0;
  margin: 0.6em 0 0;
  padding: 0;
}

.prefs legend {
  /* Floating a legend returns it to normal flow so it can sit inline
     with the radio row. */
  float: inline-start;
  padding: 0;
  min-inline-size: 6em;
  opacity: 0.6;
}

.prefs label {
  display: inline-flex;
  align-items: baseline;
  gap: 0.35em;
  min-inline-size: 6em;
  cursor: pointer;
}

/* ---- Measure and typesetting ---------------------------------------- */

body {
  max-width: 38rem;
  margin-inline: auto;
  padding: 1.75rem 1rem 4rem;
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
  margin-block: 2.2em 0.6em;
}

h1 {
  font-size: 1.5rem;
}

h2 {
  font-size: 1.2rem;
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
  margin-inline: 0;
  padding-inline-start: 1.2em;
  border-inline-start: 2px solid;
}

hr {
  border: 0;
  border-block-start: 1px solid;
  margin-block: 3em;
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
  opacity: 0.7;
}

pre {
  overflow-x: auto;
  line-height: 1.6;
  tab-size: 4;
  padding: 0.9em 1.1em;
  border: 1px solid;
}

pre,
code {
  font-size: 0.92em;
}

/* ---- Code highlighting ------------------------------------------------- */
/* The markup carries hl-* classes; what they look like is this stylesheet''s
   choice alone. The default is monochrome — weight and slant, no color — so
   it holds in light and dark alike. Add colors here if you want them. */

.prose pre .hl-comment {
  font-style: italic;
  opacity: 0.6;
}

.prose pre .hl-keyword,
.prose pre .hl-storage {
  font-weight: 700;
}

.prose pre .hl-entity {
  font-weight: 600;
}

.prose pre .hl-string {
  opacity: 0.75;
}

/* ---- Tables ---------------------------------------------------------- */

table {
  border-collapse: collapse;
}

th,
td {
  text-align: left;
  vertical-align: baseline;
  padding: 0.25em 1.5em 0.25em 0;
}

th:last-child,
td:last-child {
  padding-inline-end: 0;
}

thead th {
  border-block-end: 1px solid;
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

.tags {
  font-size: 0.85em;
}

/* ---- Header and footer: one baseline-aligned line each ---------------- */

.site-header {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  column-gap: 1.5em;
  row-gap: 0.3em;
  margin-block-end: 3.5em;
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
  padding-block-start: 1em;
  border-block-start: 1px solid;
  font-size: 0.9em;
}

/* ---- Home ------------------------------------------------------------ */

.hero {
  margin-block-end: 3em;
}

.hero h1 {
  margin-block: 0 0.3em;
}

.hero p {
  margin-block: 0;
}

/* Each entry is a grid: the date forms an aligned column (tabular numerals
   keep every row the same width), everything else hangs off the title
   column. */
.post-list article {
  display: grid;
  grid-template-columns: max-content 1fr;
  column-gap: 1.4em;
  align-items: baseline;
  margin-block: 1.6em;
}

.post-list time {
  grid-column: 1;
  font-size: 0.85em;
  opacity: 0.7;
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

.post-list p {
  font-size: 0.9em;
}

@media (max-width: 30rem) {
  .post-list article {
    grid-template-columns: 1fr;
    row-gap: 0.1em;
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

.archive-year h2 {
  font-size: 1rem;
  margin-block: 2em 0.4em;
  opacity: 0.7;
}

.archive-list {
  margin-block: 0;
}

.archive-list td {
  padding-block: 0.3em;
}

.archive-list time {
  font-size: 0.85em;
  opacity: 0.7;
}

/* ---- Search ------------------------------------------------------------ */

.search-form {
  display: flex;
  gap: 0.5em;
  margin-block-end: 2.5em;
}

.search-form input[type="search"] {
  flex: 1;
  min-width: 0;
  font: inherit;
}

.search-form button {
  font: inherit;
}

.search-results p {
  font-size: 0.9em;
}

/* Highlights keep the browser''s mark colors; only the shape is ours. */
.search-results mark {
  padding-inline: 0.1em;
}

/* ---- Article ---------------------------------------------------------- */

.article-header {
  margin-block-end: 2.5em;
}

.article-header time {
  font-size: 0.85em;
  opacity: 0.7;
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

/* Heading self-links: invisible until the heading is hovered or the
   anchor itself is focused, then a quiet mark. */
.prose .anchor {
  text-decoration: none;
  margin-inline-start: 0.35em;
  opacity: 0;
}

.prose .anchor::after {
  content: "#";
}

.prose :is(h1, h2, h3, h4, h5, h6):hover .anchor,
.prose .anchor:focus-visible {
  opacity: 0.5;
}

/* Footnotes read as an appendix: smaller, set off by a rule. */
.prose .footnotes {
  margin-block-start: 3em;
  padding-block-start: 1em;
  border-block-start: 1px solid;
  font-size: 0.85em;
}

.prose .footnote-ref a {
  text-decoration: none;
}

.prose .footnote-backref {
  text-decoration: none;
}

/* ---- Older / newer navigation ----------------------------------------- */

.post-nav {
  display: flex;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: 0.5em 2em;
  margin-block-start: 3em;
  padding-block-start: 1em;
  border-block-start: 1px solid;
  font-size: 0.9em;
}

/* One quarter-strength color is applied after every border shorthand and
   longhand, so none of those declarations can reset it to currentcolor. */
hr,
thead th,
blockquote,
pre,
.site-footer,
.post-nav,
.prose .footnotes {
  border-color: color-mix(in srgb, currentcolor 25%, transparent);
}

.post-nav-older {
  margin-inline-start: auto;
  text-align: end;
}

/* ---- Likes ------------------------------------------------------------ */
/* A native button, exactly as the browser draws it. */

.like {
  margin-block-start: 3em;
}

.like-button {
  font: inherit;
}

/* ---- Not found -------------------------------------------------------- */

.not-found {
  margin-block-start: 4em;
}

.not-found .eyebrow {
  font-variant-numeric: tabular-nums;
  letter-spacing: 0.2em;
  opacity: 0.6;
  margin-block-end: 0;
}

.not-found h1 {
  margin-block-start: 0.2em;
}

/* ---- Accessibility ---------------------------------------------------- */

.skip-link {
  position: absolute;
  left: -9999px;
}

.skip-link:focus {
  position: static;
}
' WHERE custom_css = '/* Simple Blog default theme.
   Ordinary custom CSS: edit it, replace it, or empty it in Settings.
   It concerns itself with layout only — measure, rhythm, and alignment.
   Colors, backgrounds, and decoration are left entirely to the browser;
   the few visible strokes below are drawn in the text''s own currentColor. */

:root {
  color-scheme: light dark;
}

/* ---- Reader preferences ----------------------------------------------- */
/* Taste differs per person, so measure, text size, and scheme belong to the
   visitor: prefs.js stores each reader''s choice in their browser and mirrors
   it as data attributes on the html element, which these hooks pick up. Without
   JavaScript the defaults simply hold. */

:root[data-measure="narrow"] body {
  max-width: 32rem;
}

:root[data-measure="wide"] body {
  max-width: 46rem;
}

:root[data-text="small"] body {
  font-size: 0.875rem;
}

:root[data-text="large"] body {
  font-size: 1.125rem;
}

:root[data-scheme="light"] {
  color-scheme: only light;
}

:root[data-scheme="dark"] {
  color-scheme: only dark;
}

.prefs {
  margin-block-start: 2.5em;
  font-size: 0.85em;
}

.prefs summary {
  cursor: pointer;
  opacity: 0.7;
}

.prefs fieldset {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  column-gap: 1em;
  row-gap: 0.2em;
  border: 0;
  margin: 0.6em 0 0;
  padding: 0;
}

.prefs legend {
  /* Floating a legend returns it to normal flow so it can sit inline
     with the radio row. */
  float: inline-start;
  padding: 0;
  min-inline-size: 5em;
  opacity: 0.7;
}

.prefs label {
  display: inline-flex;
  align-items: baseline;
  gap: 0.35em;
  cursor: pointer;
}

/* ---- Measure and typesetting ---------------------------------------- */

body {
  max-width: 38rem;
  margin-inline: auto;
  padding: 1.75rem 1rem 4rem;
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
  margin-block: 2.2em 0.6em;
}

h1 {
  font-size: 1.5rem;
}

h2 {
  font-size: 1.2rem;
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
  margin-inline: 0;
  padding-inline-start: 1.2em;
  border-inline-start: 2px solid;
  opacity: 0.85;
}

hr {
  border: 0;
  border-block-start: 1px solid;
  opacity: 0.25;
  margin-block: 3em;
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
  opacity: 0.75;
}

pre {
  overflow-x: auto;
  line-height: 1.6;
  tab-size: 4;
  padding: 0.9em 1.1em;
  border: 1px solid;
  border-radius: 4px;
}

pre,
code {
  font-size: 0.92em;
}

/* The border above is structural, not decorative: fade it well back. */
pre {
  border-color: color-mix(in srgb, currentColor 25%, transparent);
}

/* ---- Code highlighting ------------------------------------------------- */
/* The markup carries hl-* classes; what they look like is this stylesheet''s
   choice alone. The default is monochrome — weight and slant, no color — so
   it holds in light and dark alike. Add colors here if you want them. */

.prose pre .hl-comment {
  font-style: italic;
  opacity: 0.6;
}

.prose pre .hl-keyword,
.prose pre .hl-storage {
  font-weight: 700;
}

.prose pre .hl-entity {
  font-weight: 600;
}

.prose pre .hl-string {
  opacity: 0.75;
}

/* ---- Tables ---------------------------------------------------------- */

table {
  border-collapse: collapse;
}

th,
td {
  text-align: left;
  vertical-align: baseline;
  padding: 0.25em 1.5em 0.25em 0;
}

th:last-child,
td:last-child {
  padding-inline-end: 0;
}

thead th {
  border-block-end: 1px solid;
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

.tags {
  font-size: 0.85em;
}

/* ---- Header and footer: one baseline-aligned line each ---------------- */

.site-header {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  column-gap: 1.5em;
  row-gap: 0.3em;
  margin-block-end: 3.5em;
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
  padding-block-start: 1em;
  border-block-start: 1px solid;
  font-size: 0.9em;
}

.site-footer {
  border-color: color-mix(in srgb, currentColor 25%, transparent);
}

/* ---- Home ------------------------------------------------------------ */

.hero {
  margin-block-end: 3em;
}

.hero h1 {
  margin-block: 0 0.3em;
}

.hero p {
  margin-block: 0;
  opacity: 0.8;
}

/* Each entry is a grid: the date forms an aligned column, everything
   else hangs off the title column. */
.post-list article {
  display: grid;
  grid-template-columns: max-content 1fr;
  column-gap: 1.4em;
  align-items: baseline;
  margin-block: 1.6em;
}

.post-list time {
  grid-column: 1;
  font-size: 0.85em;
  opacity: 0.7;
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

.post-list p {
  font-size: 0.92em;
  opacity: 0.85;
}

@media (max-width: 30rem) {
  .post-list article {
    grid-template-columns: 1fr;
    row-gap: 0.1em;
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

.archive-year h2 {
  font-size: 1rem;
  margin-block: 2em 0.4em;
  opacity: 0.7;
}

.archive-list {
  margin-block: 0;
}

.archive-list td {
  padding-block: 0.3em;
}

.archive-list time {
  font-size: 0.85em;
  opacity: 0.7;
}

/* ---- Search ------------------------------------------------------------ */

.search-form {
  display: flex;
  gap: 0.5em;
  margin-block-end: 2.5em;
}

.search-form input[type="search"] {
  flex: 1;
  min-width: 0;
  font: inherit;
  padding: 0.25em 0.6em;
}

.search-form button {
  font: inherit;
  padding: 0.25em 1em;
  cursor: pointer;
}

.search-results p {
  font-size: 0.92em;
  opacity: 0.85;
}

/* Highlights keep the browser''s mark colors; only the shape is ours. */
.search-results mark {
  padding-inline: 0.1em;
}

/* ---- Article ---------------------------------------------------------- */

.article-header {
  margin-block-end: 2.5em;
}

.article-header time {
  font-size: 0.85em;
  opacity: 0.7;
}

.article-header h1 {
  margin-block: 0.2em 0.4em;
}

.article-header .dek {
  margin-block: 0;
  opacity: 0.8;
}

.cover {
  margin-block: 2em;
}

.prose :target {
  scroll-margin-block-start: 2em;
}

/* Heading self-links: invisible until the heading is hovered or the
   anchor itself is focused, then a quiet mark. */
.prose .anchor {
  text-decoration: none;
  margin-inline-start: 0.35em;
  opacity: 0;
}

.prose .anchor::after {
  content: "#";
}

.prose :is(h1, h2, h3, h4, h5, h6):hover .anchor,
.prose .anchor:focus-visible {
  opacity: 0.5;
}

/* Footnotes read as an appendix: smaller, set off by a rule. */
.prose .footnotes {
  margin-block-start: 3em;
  padding-block-start: 1em;
  border-block-start: 1px solid;
  border-color: color-mix(in srgb, currentColor 25%, transparent);
  font-size: 0.85em;
}

.prose .footnote-ref a {
  text-decoration: none;
}

.prose .footnote-backref {
  text-decoration: none;
}

/* ---- Older / newer navigation ----------------------------------------- */

.post-nav {
  display: flex;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: 0.5em 2em;
  margin-block-start: 3em;
  padding-block-start: 1em;
  border-block-start: 1px solid;
  border-color: color-mix(in srgb, currentColor 25%, transparent);
  font-size: 0.9em;
}

.post-nav-older {
  margin-inline-start: auto;
  text-align: end;
}

/* ---- Likes ------------------------------------------------------------ */

.like {
  margin-block-start: 3em;
}

.like-button {
  font: inherit;
  color: inherit;
  background: none;
  border: 1px solid;
  border-color: color-mix(in srgb, currentColor 40%, transparent);
  border-radius: 999px;
  padding: 0.15em 0.9em;
  cursor: pointer;
}

.like-button:hover {
  border-color: currentColor;
}

/* ---- Not found -------------------------------------------------------- */

.not-found {
  margin-block-start: 4em;
}

.not-found .eyebrow {
  font-variant-numeric: tabular-nums;
  letter-spacing: 0.2em;
  opacity: 0.6;
  margin-block-end: 0;
}

.not-found h1 {
  margin-block-start: 0.2em;
}

/* ---- Accessibility ---------------------------------------------------- */

.skip-link {
  position: absolute;
  left: -9999px;
}

.skip-link:focus {
  position: static;
}
';
