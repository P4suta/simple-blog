const test = require("node:test");
const assert = require("node:assert/strict");

const { fold, queryDocuments, terms } = require("../static/search.js");

const documents = [
  {
    id: 1,
    slug: "both",
    title: "東京でRust",
    summary: "",
    body: "サーバーを作る",
    folded: "東京でrust  さーばーを作る",
    published: "2026-09-02",
  },
  {
    id: 2,
    slug: "body-hit",
    title: "Newer",
    summary: "",
    body: "検索エンジンの本文",
    folded: "newer  検索えんじんの本文",
    published: "2026-09-02",
  },
  {
    id: 3,
    slug: "title-hit",
    title: "検索エンジン自作記",
    summary: "",
    body: "短い本文",
    folded: "検索えんじん自作記  短い本文",
    published: "2026-09-01",
  },
];

test("browser query semantics match the Core CJK contract", () => {
  assert.equal(fold("ＲＵＳＴ サーバー"), "rust さーばー");
  assert.deepEqual(terms("東京 rust 東京"), ["東京", "rust"]);
  assert.deepEqual(
    queryDocuments(documents, "東京 ＲＵＳＴ", 50).map((item) => item.slug),
    ["both"],
  );
  assert.deepEqual(
    queryDocuments(documents, "さーばー", 50).map((item) => item.slug),
    ["both"],
  );
});

test("title matches rank above body matches and limits are deterministic", () => {
  assert.deepEqual(
    queryDocuments(documents, "検索エンジン", 50).map((item) => item.slug),
    ["title-hit", "body-hit"],
  );
  assert.deepEqual(
    queryDocuments(documents, "検索エンジン", 1).map((item) => item.slug),
    ["title-hit"],
  );
});

test("hostile query syntax remains literal data", () => {
  for (const query of ['"OR" (', "100%", "_", "a* NOT b", "<script>"]) {
    assert.doesNotThrow(() => queryDocuments(documents, query, 50));
  }
});
