/**
 * Small pure helpers for the editor. They take no DOM so `node --test` can
 * exercise them directly; admin.ts wires them to elements.
 */

/** RFC 3339 instant → the `YYYY-MM-DDTHH:MM` shape a datetime-local control wants, in local time. */
export function isoToLocalDateTime(iso: string): string {
  if (!iso) return "";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  const pad = (value: number): string => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** Local datetime-local value → RFC 3339 UTC instant, or null when empty or unparseable. */
export function localDateTimeToIso(local: string): string | null {
  if (!local.trim()) return null;
  // ECMA-262 reads a date-time without an offset as local time.
  const date = new Date(local);
  if (Number.isNaN(date.getTime())) return null;
  return date.toISOString().replace(/\.\d{3}Z$/, "Z");
}

/** A short local time for the "saved at" indicator. */
export function formatLocalTime(date: Date, language: string): string {
  try {
    return new Intl.DateTimeFormat(language || undefined, { timeStyle: "short" }).format(date);
  } catch {
    return date.toTimeString().slice(0, 5);
  }
}

/** A local date and time for dashboard stamps. */
export function formatLocalDateTime(date: Date, language: string): string {
  try {
    return new Intl.DateTimeFormat(language || undefined, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(date);
  } catch {
    return date.toISOString();
  }
}

export interface TextCount {
  chars: number;
  words: number;
}

/**
 * Characters are counted by code point; words by Intl.Segmenter when the
 * runtime has it (so 日本語 counts 単語 rather than one giant run), otherwise
 * by runs of letters and digits.
 */
export function countText(text: string, segmenter?: { segment(input: string): Iterable<{ segment: string; isWordLike?: boolean }> } | null): TextCount {
  const chars = [...text].length;
  if (segmenter) {
    let words = 0;
    for (const part of segmenter.segment(text)) {
      if (part.isWordLike) words += 1;
    }
    return { chars, words };
  }
  const matches = text.match(/[\p{L}\p{N}]+/gu);
  return { chars, words: matches ? matches.length : 0 };
}

/** Maps an HTTP status (0 for a network failure) to a localized message key suffix. */
export function failureKey(status: number): string {
  if (status === 0) return "error_offline";
  if (status === 401 || status === 403) return "error_session";
  if (status === 409) return "conflict";
  if (status === 413) return "error_too_large";
  if (status === 422) return "error_invalid";
  if (status === 429) return "error_rate_limited";
  return "error_server";
}
