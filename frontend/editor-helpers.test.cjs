const test = require("node:test");
const assert = require("node:assert/strict");

const {
  countText,
  failureKey,
  formatLocalTime,
  isoToLocalDateTime,
  localDateTimeToIso,
} = require("./editor-helpers.ts");

test("datetime-local values round-trip through RFC 3339 in the local zone", () => {
  const local = "2026-09-03T12:01";
  const iso = localDateTimeToIso(local);

  assert.equal(iso, new Date(local).toISOString().replace(/\.\d{3}Z$/, "Z"));
  assert.match(iso, /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/);
  assert.equal(isoToLocalDateTime(iso), local);
  assert.equal(localDateTimeToIso(""), null);
  assert.equal(localDateTimeToIso("   "), null);
  assert.equal(localDateTimeToIso("not a date"), null);
  assert.equal(isoToLocalDateTime(""), "");
  assert.equal(isoToLocalDateTime("garbage"), "");
});

test("text counting understands CJK through a segmenter and falls back to letter runs", () => {
  assert.deepEqual(countText("日本語 hello world", null), { chars: 15, words: 3 });
  assert.deepEqual(countText("", null), { chars: 0, words: 0 });
  assert.deepEqual(countText("  --  ", null), { chars: 6, words: 0 });

  const segmenter = {
    segment(input) {
      return [
        { segment: "日本語", isWordLike: true },
        { segment: " ", isWordLike: false },
        { segment: "hello", isWordLike: true },
        { segment: " ", isWordLike: false },
        { segment: "world", isWordLike: true },
      ].filter((part) => input.includes(part.segment));
    },
  };
  assert.deepEqual(countText("日本語 hello world", segmenter), { chars: 15, words: 3 });

  if (typeof Intl !== "undefined" && "Segmenter" in Intl) {
    const real = new Intl.Segmenter(undefined, { granularity: "word" });
    const counted = countText("今日は良い天気です。", real);
    assert.equal(counted.chars, 10);
    assert.ok(counted.words >= 3, `expected several words, got ${counted.words}`);
  }
});

test("every failure status maps to a human message key", () => {
  assert.equal(failureKey(0), "error_offline");
  assert.equal(failureKey(401), "error_session");
  assert.equal(failureKey(403), "error_session");
  assert.equal(failureKey(409), "conflict");
  assert.equal(failureKey(413), "error_too_large");
  assert.equal(failureKey(422), "error_invalid");
  assert.equal(failureKey(429), "error_rate_limited");
  assert.equal(failureKey(500), "error_server");
  assert.equal(failureKey(502), "error_server");
});

test("saved-at times are short and never throw for an unknown language", () => {
  const date = new Date(2026, 8, 3, 9, 5);
  assert.match(formatLocalTime(date, "ja"), /9:05|09:05/);
  assert.match(formatLocalTime(date, ""), /9:05|09:05/);
  assert.match(formatLocalTime(date, "zz-INVALID-@@"), /9:05|09:05/);
});
