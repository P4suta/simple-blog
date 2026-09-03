// A safety net for the text between two autosaves: what the writer typed is
// mirrored into the browser's storage and offered back when the page loads
// with a newer local copy than the server has. Pure functions here; the
// editor wires them to the DOM. Only erasable TypeScript, so Node can run the
// tests by stripping types.

export interface LocalDraft {
  title: string;
  body: string;
  slug: string;
  summary: string;
  tags: string;
  savedAt: string;
  version: number | null;
}

export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

export const DRAFT_KEY_PREFIX = "sb:draft:";

export function draftKey(id: string | number | null | undefined): string {
  return id === null || id === undefined || id === "" ? `${DRAFT_KEY_PREFIX}new` : `${DRAFT_KEY_PREFIX}${id}`;
}

export function serializeDraft(draft: LocalDraft): string {
  return JSON.stringify(draft);
}

/** A stored draft, or null for anything that is not one (bad JSON, wrong shape). */
export function parseDraft(raw: string | null): LocalDraft | null {
  if (!raw) return null;
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null) return null;
  const record = value as Record<string, unknown>;
  const strings = ["title", "body", "slug", "summary", "tags", "savedAt"];
  if (!strings.every((key) => typeof record[key] === "string")) return null;
  if (record.version !== null && typeof record.version !== "number") return null;
  return {
    title: record.title as string,
    body: record.body as string,
    slug: record.slug as string,
    summary: record.summary as string,
    tags: record.tags as string,
    savedAt: record.savedAt as string,
    version: record.version as number | null,
  };
}

/**
 * Offer a restore only when the local copy is newer than the server's last
 * save, actually differs, and the piece can still be edited.
 */
export function shouldOfferRestore(
  local: LocalDraft | null,
  server: { updatedAt: string; body: string; trashed: boolean },
): boolean {
  if (!local || server.trashed) return false;
  const localAt = Date.parse(local.savedAt);
  if (Number.isNaN(localAt)) return false;
  const serverAt = Date.parse(server.updatedAt);
  const newer = Number.isNaN(serverAt) ? true : localAt > serverAt;
  return newer && local.body !== server.body;
}

/** Moves the entry for an unsaved piece under its new id; tolerant of a failing store. */
export function migrateDraftKey(storage: StorageLike, from: string, to: string): void {
  if (from === to) return;
  try {
    const raw = storage.getItem(from);
    if (raw === null) return;
    storage.setItem(to, raw);
    storage.removeItem(from);
  } catch {
    /* quota or private mode: the copy simply is not carried over */
  }
}

export interface DraftStore {
  read(): LocalDraft | null;
  write(draft: LocalDraft): void;
  clear(): void;
  moveTo(key: string): void;
}

export function createDraftStore(storage: StorageLike, initialKey: string): DraftStore {
  let key = initialKey;
  return {
    read() {
      try {
        return parseDraft(storage.getItem(key));
      } catch {
        return null;
      }
    },
    write(draft) {
      try {
        storage.setItem(key, serializeDraft(draft));
      } catch {
        /* nothing to do: the server copy is the durable one */
      }
    },
    clear() {
      try {
        storage.removeItem(key);
      } catch {
        /* ignore */
      }
    },
    moveTo(next) {
      migrateDraftKey(storage, key, next);
      key = next;
    },
  };
}
