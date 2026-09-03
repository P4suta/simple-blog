const assert = require("node:assert/strict");
const { test } = require("node:test");
const { insertFence, togglePrefix, toggleWrap } = require("./markdown-commands.ts");

const apply = (doc, edit) => doc.slice(0, edit.from) + edit.insert + doc.slice(edit.to);

test("wrapping a selection adds the marker and selects the inside", () => {
  const doc = "make it bold";
  const edit = toggleWrap(doc, 8, 12, "**");
  assert.equal(apply(doc, edit), "make it **bold**");
  assert.equal(doc.slice(8, 12), "bold");
  assert.deepEqual([edit.selectFrom, edit.selectTo], [10, 14]);
});

test("wrapping again removes the marker, from inside or outside the selection", () => {
  const wrapped = "make it **bold**";
  const inside = toggleWrap(wrapped, 10, 14, "**");
  assert.equal(apply(wrapped, inside), "make it bold");
  const outside = toggleWrap(wrapped, 8, 16, "**");
  assert.equal(apply(wrapped, outside), "make it bold");
  assert.deepEqual([outside.selectFrom, outside.selectTo], [8, 12]);
});

test("an empty selection wraps the word at the cursor, including 日本語 and emoji", () => {
  const doc = "強調したい語 here";
  const edit = toggleWrap(doc, 2, 2, "*");
  assert.equal(apply(doc, edit), "*強調したい語* here");
  // An emoji is not a word character; the word starts after it.
  const emoji = "say 😀hi now";
  const wrapped = toggleWrap(emoji, 6, 6, "`");
  assert.equal(apply(emoji, wrapped), "say 😀`hi` now");
});

test("with no word at the cursor the markers are inserted around it", () => {
  const doc = "a  b";
  const edit = toggleWrap(doc, 2, 2, "**");
  assert.equal(apply(doc, edit), "a **** b");
  assert.deepEqual([edit.selectFrom, edit.selectTo], [4, 4]);
});

test("line prefixes toggle on every touched line and headings replace each other", () => {
  const doc = "one\ntwo\nthree";
  const quoted = togglePrefix(doc, 1, 6, "> ");
  assert.equal(apply(doc, quoted), "> one\n> two\nthree");
  const unquoted = togglePrefix(apply(doc, quoted), 0, 9, "> ");
  assert.equal(apply(apply(doc, quoted), unquoted), "one\ntwo\nthree");
  const mixed = togglePrefix("- a\nb", 0, 4, "- ");
  assert.equal(apply("- a\nb", mixed), "- a\n- b");
  const h2 = togglePrefix("# Title", 3, 3, "## ");
  assert.equal(apply("# Title", h2), "## Title");
  const off = togglePrefix("## Title", 0, 0, "## ");
  assert.equal(apply("## Title", off), "Title");
});

test("a fence wraps the selection on its own lines or opens an empty block", () => {
  const doc = "text\ncode here\nmore";
  const edit = insertFence(doc, 5, 14);
  assert.equal(apply(doc, edit), "text\n```\ncode here\n```\nmore");
  const empty = insertFence("para", 4, 4);
  assert.equal(apply("para", empty), "para\n```\n\n```");
  assert.equal(empty.selectFrom, 4 + 1 + 4);
});
