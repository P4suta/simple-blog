// Suggestions for the comma-separated tag field: the last token is what the
// writer is typing, everything before it is settled. Pure functions so
// `node --test` can check the rules; admin.ts draws the list.

export interface SplitTags {
  /** Tags already completed, trimmed, in order. */
  done: string[];
  /** The token under the cursor, trimmed. */
  current: string;
}

export function splitTags(value: string): SplitTags {
  const parts = value.split(",").map((part) => part.trim());
  const current = parts.pop() ?? "";
  return { done: parts.filter((part) => part !== ""), current };
}

/**
 * Known tags (most used first) that could complete the current token, minus
 * the ones already on the piece. An empty token offers the most used tags.
 */
export function suggestTags(value: string, known: string[], limit = 6): string[] {
  const { done, current } = splitTags(value);
  const taken = new Set(done.map((tag) => tag.toLowerCase()));
  const needle = current.toLowerCase();
  const seen = new Set<string>();
  const suggestions: string[] = [];
  for (const tag of known) {
    const folded = tag.toLowerCase();
    if (taken.has(folded) || seen.has(folded) || folded === needle) continue;
    if (!folded.startsWith(needle)) continue;
    seen.add(folded);
    suggestions.push(tag);
    if (suggestions.length === limit) break;
  }
  return suggestions;
}

/** The field's value once `name` replaces the token under the cursor, ready for the next tag. */
export function applySuggestion(value: string, name: string): string {
  const { done } = splitTags(value);
  return `${[...done, name].join(", ")}, `;
}
