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

/**
 * RFC 3339 instant → `YYYY-MM-DDTHH:MM` on the clock of `zone`, an IANA name.
 * The site's clock is what the scheduling control shows; an unknown zone
 * falls back to the device's own.
 */
export function isoToZonedDateTime(iso: string, zone: string): string {
  if (!iso) return "";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  const parts = zonedParts(date, zone);
  if (!parts) return isoToLocalDateTime(iso);
  const pad = (value: number): string => String(value).padStart(2, "0");
  return `${parts.year}-${pad(parts.month)}-${pad(parts.day)}T${pad(parts.hour)}:${pad(parts.minute)}`;
}

/**
 * `YYYY-MM-DDTHH:MM` read on the clock of `zone` → RFC 3339 UTC instant, or
 * null when empty or unparseable. Used only to label the button before the
 * save; the server's reading of the same value is the one that counts, so a
 * minute that a clock change skips or repeats is left to it.
 */
export function zonedDateTimeToIso(local: string, zone: string): string | null {
  if (!local.trim()) return null;
  const asUtc = new Date(`${local.trim()}Z`);
  if (Number.isNaN(asUtc.getTime())) return null;
  const guess = offsetMillis(asUtc, zone);
  if (guess === null) return localDateTimeToIso(local);
  // The offset must be read at the instant itself, not at `local` read as
  // UTC: the two can lie on opposite sides of a clock change, up to fourteen
  // hours apart. One correction lands on the right side.
  const candidate = new Date(asUtc.getTime() - guess);
  const offset = offsetMillis(candidate, zone) ?? guess;
  return new Date(asUtc.getTime() - offset).toISOString().replace(/\.\d{3}Z$/, "Z");
}

/** The zone's offset from UTC at `instant`, in milliseconds; null when the runtime does not know the zone. */
function offsetMillis(instant: Date, zone: string): number | null {
  const clock = zonedParts(instant, zone);
  if (!clock) return null;
  const wall = Date.UTC(clock.year, clock.month - 1, clock.day, clock.hour, clock.minute, clock.second);
  return wall - instant.getTime();
}

interface WallClock {
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
  second: number;
}

/** The wall clock of `zone` at `date`, or null when the runtime does not know the zone. */
function zonedParts(date: Date, zone: string): WallClock | null {
  if (!zone) return null;
  try {
    const parts = new Intl.DateTimeFormat("en-US", {
      timeZone: zone,
      hourCycle: "h23",
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    }).formatToParts(date);
    const read = (type: string): number => Number(parts.find((part) => part.type === type)?.value);
    const clock = {
      year: read("year"),
      month: read("month"),
      day: read("day"),
      hour: read("hour"),
      minute: read("minute"),
      second: read("second"),
    };
    return Object.values(clock).some(Number.isNaN) ? null : clock;
  } catch {
    return null;
  }
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
