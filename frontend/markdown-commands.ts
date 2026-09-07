// Pure editing commands over a Markdown document: they compute a replacement
// and the selection that follows, and the editor applies both. No DOM, so
// `node --test` can exercise every rule.

export interface Edit {
  /** The span to replace. */
  from: number;
  to: number;
  insert: string;
  /** The selection afterwards. */
  selectFrom: number;
  selectTo: number;
}

const WORD = /[\p{L}\p{N}_]/u;

function wordAround(doc: string, position: number): { from: number; to: number } {
  const chars = [...doc];
  // Work in code points so a surrogate pair is never split.
  const offsets: number[] = [];
  let offset = 0;
  for (const char of chars) {
    offsets.push(offset);
    offset += char.length;
  }
  offsets.push(offset);
  let index = offsets.findIndex((at) => at >= position);
  if (index === -1) index = chars.length;
  let start = index;
  while (start > 0 && WORD.test(chars[start - 1])) start -= 1;
  let end = index;
  while (end < chars.length && WORD.test(chars[end])) end += 1;
  return { from: offsets[start], to: offsets[end] };
}

/**
 * Wraps the selection in `marker` (bold, italic, code), or unwraps it when
 * it is already wrapped. With nothing selected the word at the cursor is
 * used; with no word either, the markers are inserted around the cursor.
 */
export function toggleWrap(doc: string, from: number, to: number, marker: string): Edit {
  let start = from;
  let end = to;
  if (start === end) {
    const word = wordAround(doc, start);
    start = word.from;
    end = word.to;
  }
  const selected = doc.slice(start, end);
  const length = marker.length;
  if (
    selected.length >= 2 * length &&
    selected.startsWith(marker) &&
    selected.endsWith(marker)
  ) {
    const inner = selected.slice(length, selected.length - length);
    return { from: start, to: end, insert: inner, selectFrom: start, selectTo: start + inner.length };
  }
  if (doc.slice(start - length, start) === marker && doc.slice(end, end + length) === marker) {
    return {
      from: start - length,
      to: end + length,
      insert: selected,
      selectFrom: start - length,
      selectTo: start - length + selected.length,
    };
  }
  return {
    from: start,
    to: end,
    insert: `${marker}${selected}${marker}`,
    selectFrom: start + length,
    selectTo: start + length + selected.length,
  };
}

const HEADING = /^#{1,6} /;

/**
 * Adds `prefix` to every line the selection touches, or removes it when every
 * one of them already carries it. A heading prefix replaces another heading
 * level instead of stacking on it.
 */
export function togglePrefix(doc: string, from: number, to: number, prefix: string): Edit {
  const lineStart = doc.lastIndexOf("\n", from - 1) + 1;
  const newlineAfter = doc.indexOf("\n", to);
  const lineEnd = newlineAfter === -1 ? doc.length : newlineAfter;
  const lines = doc.slice(lineStart, lineEnd).split("\n");
  const heading = HEADING.test(prefix);
  const allPrefixed = lines.every((line) => line.startsWith(prefix));
  const changed = lines.map((line) => {
    if (allPrefixed) return line.slice(prefix.length);
    if (heading) return prefix + line.replace(HEADING, "");
    return line.startsWith(prefix) ? line : prefix + line;
  });
  const insert = changed.join("\n");
  return { from: lineStart, to: lineEnd, insert, selectFrom: lineStart, selectTo: lineStart + insert.length };
}

/** Wraps the selection in a fenced code block, or opens an empty fence at the cursor. */
/**
 * Every key binding of the editor, in the order the help lists them. `key`
 * is CodeMirror's notation; `labelKey` names the localized description. The
 * keymap and the help are both built from this table, so neither can fall
 * behind the other. Save is bound on the document (it works from every
 * field) but belongs in the list.
 */
export const EDITOR_SHORTCUTS: readonly { key: string; labelKey: string }[] = [
  { key: "Mod-s", labelKey: "editor.shortcut_save" },
  { key: "Mod-b", labelKey: "editor.shortcut_bold" },
  { key: "Mod-i", labelKey: "editor.shortcut_italic" },
  { key: "Mod-`", labelKey: "editor.shortcut_code" },
  { key: "Mod-k", labelKey: "editor.shortcut_link" },
  { key: "Mod-Alt-1", labelKey: "editor.shortcut_heading1" },
  { key: "Mod-Alt-2", labelKey: "editor.shortcut_heading2" },
  { key: "Mod-Alt-3", labelKey: "editor.shortcut_heading3" },
  { key: "Mod-Shift-q", labelKey: "editor.shortcut_quote" },
  { key: "Mod-Shift-l", labelKey: "editor.shortcut_list" },
  { key: "Mod-Alt-c", labelKey: "editor.shortcut_fence" },
  { key: "Mod-Shift-p", labelKey: "editor.shortcut_preview" },
  { key: "Mod-Shift-f", labelKey: "editor.shortcut_focus" },
];

/** A binding the way a keyboard shows it: `Mod-Shift-p` with `⌘` → `⌘+Shift+P`. */
export function describeShortcut(key: string, mod: string): string {
  return key
    .split("-")
    .map((part) => (part === "Mod" ? mod : part.length === 1 ? part.toUpperCase() : part))
    .join("+");
}

export function insertFence(doc: string, from: number, to: number): Edit {
  const selected = doc.slice(from, to);
  const before = from > 0 && doc[from - 1] !== "\n" ? "\n" : "";
  const after = to < doc.length && doc[to] !== "\n" ? "\n" : "";
  if (selected) {
    const insert = `${before}\`\`\`\n${selected}\n\`\`\`${after}`;
    return { from, to, insert, selectFrom: from + before.length + 3, selectTo: from + before.length + 3 };
  }
  const insert = `${before}\`\`\`\n\n\`\`\`${after}`;
  const cursor = from + before.length + 4;
  return { from, to, insert, selectFrom: cursor, selectTo: cursor };
}
