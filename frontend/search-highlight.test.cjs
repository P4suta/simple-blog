const assert = require("node:assert/strict");
const { test } = require("node:test");
const { highlight, snippet, terms } = require("../static/search.js");

const hits = (segments) => segments.filter((segment) => segment.hit).map((segment) => segment.text);
const join = (segments) => segments.map((segment) => segment.text).join("");

test("highlighting maps folded matches back to the katakana source", () => {
  const segments = highlight("検索エンジン自作記", terms("検索えんじん"));
  assert.deepEqual(segments, [
    { text: "検索エンジン", hit: true },
    { text: "自作記", hit: false },
  ]);
});

test("full-width and compatibility characters highlight the source characters", () => {
  assert.deepEqual(hits(highlight("ＲＵＳＴで書く", terms("rust"))), ["ＲＵＳＴ"]);
  assert.deepEqual(hits(highlight("㍿東京", terms("株式"))), ["㍿"]);
  assert.deepEqual(hits(highlight("Rust and rust", terms("RUST"))), ["Rust", "rust"]);
});

test("overlapping and adjacent terms merge into one mark", () => {
  const segments = highlight("検索エンジン", ["検索", "検索えんじん", "えんじん"]);
  assert.deepEqual(segments, [{ text: "検索エンジン", hit: true }]);
  assert.equal(join(highlight("abc", ["ab", "bc"])), "abc");
  assert.deepEqual(hits(highlight("abc", ["ab", "bc"])), ["abc"]);
});

test("a snippet centres on the first body match and clips with ellipses", () => {
  const body = `${"あ".repeat(300)}検索エンジン${"い".repeat(300)}`;
  const segments = snippet(body, terms("検索えんじん"), 60);
  assert.equal(segments[0].text, "…");
  assert.equal(segments[segments.length - 1].text, "…");
  const middle = segments.slice(1, -1);
  assert.equal([...join(middle)].length, 60);
  assert.deepEqual(hits(middle), ["検索エンジン"]);
});

test("without a match the snippet is the opening of the text", () => {
  assert.deepEqual(snippet("short", terms("zzz")), [{ text: "short", hit: false }]);
  const long = snippet("x".repeat(400), terms("zzz"));
  assert.equal(join(long.slice(0, -1)).length, 120);
  assert.equal(long[long.length - 1].text, "…");
});

test("hostile input stays literal text", () => {
  const segments = highlight("<b>x</b> & \"q\"", terms("<b>"));
  assert.equal(join(segments), "<b>x</b> & \"q\"");
  assert.deepEqual(hits(segments), ["<b>"], "the closing tag is </b>, not a match");
  for (const hostile of ["\"OR\" (", "100%", "_", "a* NOT b", "<script>", "\\u{FFFF}"]) {
    assert.doesNotThrow(() => snippet("body text", terms(hostile)));
    assert.doesNotThrow(() => highlight(hostile, terms(hostile)));
  }
});
