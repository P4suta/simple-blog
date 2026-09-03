const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const {
  conflictNavigationParameters,
  isConflictPage,
  postFormAsNavigation,
} = require("./form-navigation.ts");
const documentStreamCall =
  /document\s*(?:\.|\?\.)\s*(?:open|write|writeln)\s*(?:\?\.)?\s*\(/;

test("native conflict navigation survives a named submit control", (context) => {
  const forms = [];
  const appended = [];
  let submissions = 0;
  class FakeHTMLFormElement {
    constructor() {
      this.action = "";
      this.method = "";
      this.hidden = false;
      this.children = [];
    }

    append(child) {
      this.children.push(child);
      if (child.name === "submit") this.submit = child;
    }

    submit() {
      submissions += 1;
    }
  }
  const previousHTMLFormElement = globalThis.HTMLFormElement;
  globalThis.HTMLFormElement = FakeHTMLFormElement;
  context.after(() => {
    if (previousHTMLFormElement === undefined) delete globalThis.HTMLFormElement;
    else globalThis.HTMLFormElement = previousHTMLFormElement;
  });
  const document = {
    createElement(tag) {
      if (tag === "form") {
        const form = new FakeHTMLFormElement();
        forms.push(form);
        return form;
      }
      if (tag === "input") return { type: "", name: "", value: "" };
      throw new Error(`unexpected element: ${tag}`);
    },
    body: {
      append(element) {
        appended.push(element);
      },
    },
  };
  const parameters = new URLSearchParams([
    ["csrf", "opaque"],
    ["title", "Second tab unsaved"],
    ["summary", '<img src=x onerror="globalThis.compromised=true">'],
    ["tags", "rust"],
    ["tags", "cms"],
    ["intent", "autosave"],
    ["submit", "future-field"],
  ]);

  postFormAsNavigation(document, "/admin/content/7/", parameters);

  assert.equal(forms.length, 1);
  assert.equal(forms[0].action, "/admin/content/7/");
  assert.equal(forms[0].method, "post");
  assert.equal(forms[0].hidden, true);
  assert.deepEqual(appended, [forms[0]]);
  assert.deepEqual(
    forms[0].children.map(({ type, name, value }) => [type, name, value]),
    [
      ["hidden", "csrf", "opaque"],
      ["hidden", "title", "Second tab unsaved"],
      ["hidden", "summary", '<img src=x onerror="globalThis.compromised=true">'],
      ["hidden", "tags", "rust"],
      ["hidden", "tags", "cms"],
      ["hidden", "intent", "autosave"],
      ["hidden", "submit", "future-field"],
    ],
  );
  assert.equal(typeof forms[0].submit, "object");
  assert.equal(submissions, 1);
});

test("editor conflict handling uses native navigation instead of an HTML string sink", () => {
  const source = fs.readFileSync(path.join(__dirname, "admin.ts"), "utf8");

  assert.doesNotMatch(source, documentStreamCall);
  assert.match(
    source,
    /postFormAsNavigation\(\s*document,\s*editor\.action,\s*conflictNavigationParameters\(parameters\),?\s*\)/,
  );
  assert.match(source, /isConflictPage\(response\.status, response\.headers\.get\("content-type"\)\)/);
});

test("only the server's HTML conflict page may replace the editor", () => {
  assert.equal(isConflictPage(409, "text/html; charset=utf-8"), true);
  assert.equal(isConflictPage(409, "TEXT/HTML"), true);
  // A taken slug is also a 409, but as plain text: the writer's draft must stay.
  assert.equal(isConflictPage(409, "text/plain; charset=utf-8"), false);
  assert.equal(isConflictPage(409, null), false);
  assert.equal(isConflictPage(200, "text/html"), false);
  assert.equal(isConflictPage(500, "text/html"), false);
});

test("the replayed conflict request asks for the explicit HTML outcome", () => {
  const original = new URLSearchParams([
    ["csrf", "opaque"],
    ["tags", "rust"],
    ["tags", "cms"],
    ["intent", "autosave"],
  ]);

  const replay = conflictNavigationParameters(original);

  assert.equal(replay.get("intent"), "explicit");
  assert.deepEqual(replay.getAll("tags"), ["rust", "cms"]);
  assert.equal(replay.get("csrf"), "opaque");
  // The live autosave parameters are left untouched for the editor's own use.
  assert.equal(original.get("intent"), "autosave");
});

test("document stream guard recognizes direct and optional calls", () => {
  for (const source of [
    'document.write("unsafe")',
    'document?.write("unsafe")',
    'document.write?.("unsafe")',
    'document?.write?.("unsafe")',
    'document . writeln ?. ("unsafe")',
  ]) {
    assert.match(source, documentStreamCall);
  }
});
