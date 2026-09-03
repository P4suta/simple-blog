const assert = require("node:assert/strict");
const { test } = require("node:test");
const {
  createDraftStore,
  draftKey,
  migrateDraftKey,
  parseDraft,
  serializeDraft,
  shouldOfferRestore,
} = require("./draft-store.ts");

const draft = (overrides = {}) => ({
  title: "Title",
  body: "Body",
  slug: "title",
  summary: "",
  tags: "a, b",
  savedAt: "2026-09-03T10:00:00.000Z",
  version: 3,
  ...overrides,
});

const memory = () => {
  const map = new Map();
  return {
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => map.set(key, value),
    removeItem: (key) => map.delete(key),
    map,
  };
};

const throwing = {
  getItem() {
    throw new Error("blocked");
  },
  setItem() {
    throw new Error("quota");
  },
  removeItem() {
    throw new Error("blocked");
  },
};

test("draft keys are per content id and \"new\" for unsaved pieces", () => {
  assert.equal(draftKey(null), "sb:draft:new");
  assert.equal(draftKey(undefined), "sb:draft:new");
  assert.equal(draftKey(""), "sb:draft:new");
  assert.equal(draftKey(7), "sb:draft:7");
  assert.equal(draftKey("7"), "sb:draft:7");
});

test("drafts round-trip through JSON and tolerate garbage", () => {
  const original = draft({ version: null });
  assert.deepEqual(parseDraft(serializeDraft(original)), original);
  assert.equal(parseDraft("{"), null);
  assert.equal(parseDraft(null), null);
  assert.equal(parseDraft(""), null);
  assert.equal(parseDraft('{"title":1}'), null);
  assert.equal(parseDraft('"just a string"'), null);
  assert.equal(parseDraft(JSON.stringify(draft({ version: "3" }))), null);
});

test("restore is offered only for a newer, different, untrashed body", () => {
  const server = { updatedAt: "2026-09-03T09:00:00Z", body: "Body on server", trashed: false };
  assert.equal(shouldOfferRestore(draft(), server), true);
  assert.equal(shouldOfferRestore(draft({ savedAt: "2026-09-03T08:00:00Z" }), server), false);
  assert.equal(shouldOfferRestore(draft({ body: "Body on server" }), server), false);
  assert.equal(shouldOfferRestore(draft(), { ...server, trashed: true }), false);
  assert.equal(shouldOfferRestore(null, server), false);
  assert.equal(shouldOfferRestore(draft({ savedAt: "never" }), server), false);
  // A brand-new piece has no server timestamp: any local text counts.
  assert.equal(shouldOfferRestore(draft(), { updatedAt: "", body: "", trashed: false }), true);
});

test("the \"new\" entry moves to the id key after the first save", () => {
  const storage = memory();
  storage.setItem("sb:draft:new", serializeDraft(draft()));
  migrateDraftKey(storage, "sb:draft:new", "sb:draft:9");
  assert.equal(storage.getItem("sb:draft:new"), null);
  assert.deepEqual(parseDraft(storage.getItem("sb:draft:9")), draft());
  migrateDraftKey(storage, "sb:draft:missing", "sb:draft:10");
  assert.equal(storage.getItem("sb:draft:10"), null);
  migrateDraftKey(storage, "sb:draft:9", "sb:draft:9");
  assert.deepEqual(parseDraft(storage.getItem("sb:draft:9")), draft());
  assert.doesNotThrow(() => migrateDraftKey(throwing, "a", "b"));
});

test("a store writes, reads, clears, moves and survives a throwing storage", () => {
  const storage = memory();
  const store = createDraftStore(storage, draftKey(null));
  assert.equal(store.read(), null);
  store.write(draft());
  assert.deepEqual(store.read(), draft());
  store.moveTo(draftKey(4));
  assert.equal(storage.getItem("sb:draft:new"), null);
  assert.deepEqual(store.read(), draft());
  store.clear();
  assert.equal(store.read(), null);
  assert.equal(storage.getItem("sb:draft:4"), null);

  const blocked = createDraftStore(throwing, draftKey(1));
  assert.equal(blocked.read(), null);
  assert.doesNotThrow(() => blocked.write(draft()));
  assert.doesNotThrow(() => blocked.clear());
  assert.doesNotThrow(() => blocked.moveTo(draftKey(2)));
});
