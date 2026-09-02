const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const { postFormAsNavigation } = require("./form-navigation.ts");

test("native conflict navigation preserves every submitted field without evaluating HTML", () => {
  const forms = [];
  const appended = [];
  let submissions = 0;
  const document = {
    createElement(tag) {
      if (tag === "form") {
        const form = {
          action: "",
          method: "",
          hidden: false,
          children: [],
          append(child) {
            this.children.push(child);
          },
          submit() {
            submissions += 1;
          },
        };
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
    ],
  );
  assert.equal(submissions, 1);
});

test("editor conflict handling uses native navigation instead of an HTML string sink", () => {
  const source = fs.readFileSync(path.join(__dirname, "admin.ts"), "utf8");

  assert.doesNotMatch(source, /document\.(?:open|write|writeln)\s*\(/);
  assert.match(source, /postFormAsNavigation\(document, editor\.action, parameters\)/);
});
