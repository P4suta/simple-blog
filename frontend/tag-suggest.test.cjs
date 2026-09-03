const assert = require("node:assert/strict");
const { test } = require("node:test");
const { applySuggestion, splitTags, suggestTags } = require("./tag-suggest.ts");

const known = ["Rust", "Writing", "Rails", "日本語", "Rust Tooling", "R"];

test("the last comma-separated token is the one being typed", () => {
  assert.deepEqual(splitTags("Rust, Wri"), { done: ["Rust"], current: "Wri" });
  assert.deepEqual(splitTags("Rust, "), { done: ["Rust"], current: "" });
  assert.deepEqual(splitTags(""), { done: [], current: "" });
  assert.deepEqual(splitTags(" a ,, b , c"), { done: ["a", "b"], current: "c" });
});

test("suggestions complete the token, skip tags already present, and cap the list", () => {
  assert.deepEqual(suggestTags("r", known), ["Rust", "Rails", "Rust Tooling"]);
  assert.deepEqual(
    suggestTags("Rust, r", known),
    ["Rails", "Rust Tooling"],
    "a tag equal to the token is not re-offered",
  );
  assert.deepEqual(suggestTags("rust", known), ["Rust Tooling"], "an exact match is not re-offered");
  assert.deepEqual(suggestTags("日", known), ["日本語"]);
  assert.deepEqual(suggestTags("", known, 2), ["Rust", "Writing"], "an empty token offers the most used");
  assert.deepEqual(suggestTags("zzz", known), []);
  assert.deepEqual(suggestTags("r", ["Rust", "rust", "RUST"]), ["Rust"], "case variants collapse");
});

test("applying a suggestion replaces the token and opens the next slot", () => {
  assert.equal(applySuggestion("Rust, Wri", "Writing"), "Rust, Writing, ");
  assert.equal(applySuggestion("", "Rust"), "Rust, ");
  assert.equal(applySuggestion("a, b, ", "c"), "a, b, c, ");
});
