import { EditorView, basicSetup, minimalSetup } from "codemirror";
import { keymap } from "@codemirror/view";
import { EditorSelection, EditorState } from "@codemirror/state";
import { markdown } from "@codemirror/lang-markdown";
import { css } from "@codemirror/lang-css";
import {
  countText,
  failureKey,
  formatLocalDateTime,
  formatLocalTime,
  isoToLocalDateTime,
  localDateTimeToIso,
} from "./editor-helpers";

// Server stamps are UTC; readers of the dashboard live in their own zone.
for (const time of document.querySelectorAll<HTMLTimeElement>("time[data-local-time]")) {
  const date = new Date(time.dateTime);
  if (!Number.isNaN(date.getTime())) {
    time.textContent = formatLocalDateTime(date, document.documentElement.lang);
  }
}
import {
  conflictNavigationParameters,
  isConflictPage,
  postFormAsNavigation,
} from "./form-navigation";
import { type LocalDraft, createDraftStore, draftKey, shouldOfferRestore } from "./draft-store";

// Word-wise cursor movement that actually understands 日本語. CodeMirror's
// default group motion sees an unbroken CJK run as one giant word;
// Intl.Segmenter knows where 単語 and 助詞 begin and end.
const wordSegmenter =
  typeof Intl !== "undefined" && "Segmenter" in Intl
    ? new Intl.Segmenter(undefined, { granularity: "word" })
    : null;

function segmentBoundary(state: EditorState, position: number, forward: boolean): number {
  const line = state.doc.lineAt(position);
  if (forward && position >= line.to) return Math.min(position + 1, state.doc.length);
  if (!forward && position <= line.from) return Math.max(position - 1, 0);
  const offset = position - line.from;
  const segments = [...wordSegmenter!.segment(line.text)];
  if (forward) {
    for (const segment of segments) {
      const end = segment.index + segment.segment.length;
      if (end > offset && segment.isWordLike) return line.from + end;
    }
    return line.to;
  }
  for (let index = segments.length - 1; index >= 0; index -= 1) {
    const segment = segments[index];
    if (segment.index < offset && segment.isWordLike) return line.from + segment.index;
  }
  return line.from;
}

function moveBySegment(view: EditorView, forward: boolean, extend: boolean): boolean {
  if (!wordSegmenter) return false;
  const selection = EditorSelection.create(
    view.state.selection.ranges.map((range) => {
      const head = segmentBoundary(view.state, range.head, forward);
      return extend ? EditorSelection.range(range.anchor, head) : EditorSelection.cursor(head);
    }),
    view.state.selection.mainIndex,
  );
  view.dispatch({ selection, scrollIntoView: true, userEvent: "select" });
  return true;
}

function deleteSegmentBackward(view: EditorView): boolean {
  if (!wordSegmenter) return false;
  const changes = view.state.changeByRange((range) => {
    const from = range.empty ? segmentBoundary(view.state, range.head, false) : range.from;
    const to = range.empty ? range.head : range.to;
    return {
      changes: { from, to },
      range: EditorSelection.cursor(from),
    };
  });
  view.dispatch({ ...changes, scrollIntoView: true, userEvent: "delete.group" });
  return true;
}

const segmentKeymap = keymap.of([
  {
    key: "Ctrl-ArrowRight",
    mac: "Alt-ArrowRight",
    run: (view) => moveBySegment(view, true, false),
    shift: (view) => moveBySegment(view, true, true),
  },
  {
    key: "Ctrl-ArrowLeft",
    mac: "Alt-ArrowLeft",
    run: (view) => moveBySegment(view, false, false),
    shift: (view) => moveBySegment(view, false, true),
  },
  { key: "Ctrl-Backspace", mac: "Alt-Backspace", run: deleteSegmentBackward },
]);

const base64url = {
  decode(value: string): Uint8Array {
    const normalized = value.replace(/-/g, "+").replace(/_/g, "/");
    const bytes = atob(normalized);
    return Uint8Array.from(bytes, (character) => character.charCodeAt(0));
  },
  encode(value: ArrayBuffer): string {
    let binary = "";
    new Uint8Array(value).forEach((byte) => {
      binary += String.fromCharCode(byte);
    });
    return btoa(binary)
      .replace(/\+/g, "-")
      .replace(/\//g, "_")
      .replace(/=+$/g, "");
  },
};

function publicKeyOptions(options: any): any {
  const result = { ...options, challenge: base64url.decode(options.challenge) };
  if (options.user) {
    result.user = { ...options.user, id: base64url.decode(options.user.id) };
  }
  for (const field of ["excludeCredentials", "allowCredentials"]) {
    if (options[field]) {
      result[field] = options[field].map((item: any) => ({
        ...item,
        id: base64url.decode(item.id),
      }));
    }
  }
  return result;
}

function credentialJSON(credential: PublicKeyCredential): Record<string, unknown> {
  const response = credential.response as AuthenticatorAttestationResponse &
    AuthenticatorAssertionResponse;
  return {
    id: credential.id,
    rawId: base64url.encode(credential.rawId),
    type: credential.type,
    response: {
      clientDataJSON: base64url.encode(response.clientDataJSON),
      attestationObject: response.attestationObject
        ? base64url.encode(response.attestationObject)
        : undefined,
      authenticatorData: response.authenticatorData
        ? base64url.encode(response.authenticatorData)
        : undefined,
      signature: response.signature ? base64url.encode(response.signature) : undefined,
      userHandle: response.userHandle ? base64url.encode(response.userHandle) : null,
    },
    clientExtensionResults: credential.getClientExtensionResults(),
  };
}

/** A server answer the UI can explain: the status decides the sentence, the detail fills it in. */
class RequestFailure extends Error {
  constructor(
    readonly status: number,
    readonly detail: string,
  ) {
    super(detail || `HTTP ${status}`);
  }
}

async function post(url: string, data: unknown): Promise<any> {
  const response = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(data),
  });
  const contentType = response.headers.get("content-type") ?? "";
  const payload = contentType.includes("json")
    ? await response.json()
    : { error: await response.text() };
  if (!response.ok) {
    throw new RequestFailure(response.status, payload.error || "");
  }
  return payload;
}

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : "Request failed";
}

/**
 * Turns a failed request into the localized sentence for its cause. Messages
 * come from the page's data attributes; `{detail}` carries the server's own
 * words when it gave any (a validation reason, for instance).
 */
function describeFailure(reason: unknown, messages: Record<string, string | undefined>): string {
  const status = reason instanceof RequestFailure ? reason.status : 0;
  const detail =
    reason instanceof RequestFailure
      ? reason.detail
      : reason instanceof Error && !(reason instanceof TypeError)
        ? reason.message
        : "";
  const template = messages[failureKey(status)] ?? messages.error_server ?? "Request failed";
  return template.replace("{detail}", detail.trim()).replace(/[:：]\s*$/, "");
}

function failureMessages(source: DOMStringMap): Record<string, string | undefined> {
  return {
    error_session: source.msgErrorSession,
    error_invalid: source.msgErrorInvalid,
    error_too_large: source.msgErrorTooLarge,
    error_rate_limited: source.msgErrorRateLimited,
    error_server: source.msgErrorServer,
    error_offline: source.msgErrorOffline,
    conflict: source.msgErrorServer,
  };
}

async function uploadMedia(csrf: string, file: File, altText = ""): Promise<any> {
  const data = new FormData();
  data.set("csrf", csrf);
  data.set("alt_text", altText.trim() || file.name);
  data.set("file", file);
  const response = await fetch("/admin/media/", { method: "POST", body: data });
  if (!response.ok) throw new RequestFailure(response.status, await response.text());
  return response.json();
}

function wireDropZone(zone: HTMLElement, onFile: (file: File) => Promise<void>): void {
  const hint = zone.querySelector<HTMLElement>("small");
  const originalHint = hint?.textContent ?? "";
  const messages = failureMessages(document.querySelector<HTMLElement>("[data-editor], [data-settings]")?.dataset ?? {});
  const uploadFailed = document.querySelector<HTMLElement>("[data-editor], [data-settings]")?.dataset.msgUploadFailed;
  const handle = async (file: File | undefined): Promise<void> => {
    if (!file) return;
    try {
      await onFile(file);
      if (hint) {
        hint.textContent = originalHint;
        delete hint.dataset.error;
      }
    } catch (reason) {
      if (hint) {
        const detail = describeFailure(reason, messages);
        hint.textContent = uploadFailed ? uploadFailed.replace("{detail}", detail) : detail;
        hint.dataset.error = "true";
      }
    }
  };
  zone.addEventListener("dragover", (event) => {
    event.preventDefault();
    zone.dataset.dragging = "true";
  });
  zone.addEventListener("dragleave", () => delete zone.dataset.dragging);
  zone.addEventListener("drop", async (event) => {
    event.preventDefault();
    delete zone.dataset.dragging;
    await handle(event.dataTransfer?.files[0]);
  });

  // Click-to-pick lives on the hint element, not the zone: the body zone wraps
  // the whole editor, and a zone-wide click handler would steal every click.
  const picker = zone.querySelector<HTMLElement>("[data-media-pick]");
  if (!picker) return;
  const input = document.createElement("input");
  input.type = "file";
  input.accept = "image/*";
  input.hidden = true;
  zone.append(input);
  input.addEventListener("change", async () => {
    await handle(input.files?.[0] ?? undefined);
    input.value = "";
  });
  picker.setAttribute("role", "button");
  picker.tabIndex = 0;
  picker.addEventListener("click", () => input.click());
  picker.addEventListener("keydown", (event) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      input.click();
    }
  });
}

// Any [data-media-target] section is an image slot: drop to upload and fill
// the hidden input, clear to empty it. Used for covers, logos, and favicons.
const csrfMeta = document.querySelector<HTMLMetaElement>('meta[name="csrf-token"]')?.content;
document.querySelectorAll<HTMLElement>("[data-media-target]").forEach((zone) => {
  if (!csrfMeta) return;
  const input = zone.querySelector<HTMLInputElement>("input[type=hidden]")!;
  const thumb = zone.querySelector<HTMLImageElement>("[data-media-thumb]")!;
  const clear = zone.querySelector<HTMLButtonElement>("[data-media-clear]")!;
  const altInput = zone.querySelector<HTMLInputElement>("[data-cover-alt]");
  const altSave = zone.querySelector<HTMLButtonElement>("[data-cover-alt-save]");
  const altForm = document.querySelector<HTMLFormElement>("[data-cover-alt-form]");
  const showAlt = (visible: boolean): void => {
    if (altInput) altInput.closest("label")!.hidden = !visible;
    if (altSave) altSave.hidden = !visible;
  };
  clear.addEventListener("click", () => {
    input.value = "";
    thumb.hidden = true;
    thumb.removeAttribute("src");
    clear.hidden = true;
    showAlt(false);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  wireDropZone(zone, async (file) => {
    const media = await uploadMedia(csrfMeta, file, altInput?.value ?? "");
    input.value = media.id;
    thumb.src = media.url;
    thumb.hidden = false;
    clear.hidden = false;
    if (altInput) altInput.value = media.alt_text ?? "";
    if (altForm) altForm.action = `/admin/media/${media.id}/`;
    showAlt(true);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  // Alt text saves on its own, a moment after typing stops; the button
  // remains for keyboards and for browsers without scripting.
  if (altInput && altForm) {
    let timer: number | undefined;
    const saveAlt = async (): Promise<void> => {
      if (!input.value) return;
      const body = new URLSearchParams(new FormData(altForm) as unknown as Record<string, string>);
      body.set("alt_text", altInput.value);
      const response = await fetch(altForm.action, {
        method: "POST",
        headers: { Accept: "application/json" },
        body,
      });
      altInput.dataset.error = response.ok ? "" : "true";
    };
    altInput.addEventListener("input", () => {
      clearTimeout(timer);
      timer = window.setTimeout(() => void saveAlt(), 1_200);
    });
    altForm.addEventListener("submit", (event) => {
      event.preventDefault();
      clearTimeout(timer);
      void saveAlt();
    });
  }
});

function showRecoveryCodes(codes: string[], labels: DOMStringMap): void {
  const main = document.createElement("main");
  main.className = "auth-card";
  const heading = document.createElement("h1");
  heading.textContent = labels.msgRecoveryHeading ?? "Recovery codes";
  const explanation = document.createElement("p");
  explanation.textContent = labels.msgRecoveryNote ?? "Shown only once. Store them somewhere safe.";
  const output = document.createElement("pre");
  output.textContent = codes.join("\n");
  const link = document.createElement("a");
  link.className = "primary-button";
  link.href = "/admin/";
  link.textContent = labels.msgToAdmin ?? "Go to admin";
  main.append(heading, explanation, output, link);
  document.body.replaceChildren(main);
}

const setup = document.querySelector<HTMLElement>("[data-passkey-setup]");
setup?.querySelector<HTMLButtonElement>("[data-passkey-action]")?.addEventListener("click", async () => {
  const error = setup.querySelector<HTMLElement>("[data-auth-error]")!;
  try {
    error.textContent = "";
    const token = setup.dataset.setupToken!;
    const start = await post("/admin/auth/setup/start", { token });
    const credential = (await navigator.credentials.create({
      publicKey: publicKeyOptions(start.options.publicKey),
    })) as PublicKeyCredential;
    const name = setup.querySelector<HTMLInputElement>("[data-passkey-name]")!.value;
    const completed = await post("/admin/auth/setup/finish", {
      token,
      flow_id: start.flow_id,
      name,
      credential: credentialJSON(credential),
      // Offered once: a fresh site adopts the browser's zone for its dates.
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    });
    showRecoveryCodes(completed.recovery_codes, setup.dataset);
  } catch (reason) {
    error.textContent = errorMessage(reason);
  }
});

const login = document.querySelector<HTMLElement>("[data-passkey-login]");
login?.querySelector<HTMLButtonElement>("[data-passkey-action]")?.addEventListener("click", async () => {
  const error = login.querySelector<HTMLElement>("[data-auth-error]")!;
  try {
    error.textContent = "";
    const start = await post("/admin/auth/login/start", {});
    const credential = (await navigator.credentials.get({
      publicKey: publicKeyOptions(start.options.publicKey),
    })) as PublicKeyCredential;
    await post("/admin/auth/login/finish", {
      flow_id: start.flow_id,
      credential: credentialJSON(credential),
    });
    // The server has already reduced `next` to a same-site admin path.
    const next = login.dataset.next ?? "";
    location.href = next.startsWith("/admin/") && !next.startsWith("//") ? next : "/admin/";
  } catch (reason) {
    error.textContent = errorMessage(reason);
  }
});

const passkeyAdd = document.querySelector<HTMLElement>("[data-passkey-add]");
passkeyAdd
  ?.querySelector<HTMLButtonElement>("[data-passkey-action]")
  ?.addEventListener("click", async () => {
    const error = passkeyAdd.querySelector<HTMLElement>("[data-auth-error]")!;
    try {
      error.textContent = "";
      const csrf = document.querySelector<HTMLMetaElement>('meta[name="csrf-token"]')!.content;
      const start = await post("/admin/auth/passkeys/start", { csrf });
      const credential = (await navigator.credentials.create({
        publicKey: publicKeyOptions(start.options.publicKey),
      })) as PublicKeyCredential;
      await post("/admin/auth/passkeys/finish", {
        csrf,
        flow_id: start.flow_id,
        name: passkeyAdd.querySelector<HTMLInputElement>("[data-passkey-name]")!.value,
        credential: credentialJSON(credential),
      });
      location.reload();
    } catch (reason) {
      error.textContent = errorMessage(reason);
    }
  });

/**
 * A save is durable once the server answers; the public site may still lag
 * behind while a failed build is retried. The chip says which, and clears the
 * pending mark on the next save that reaches the site.
 */
function showSaved(
  chip: HTMLElement,
  site: unknown,
  savedText: string,
  pendingText: string,
): void {
  if (site === "pending") {
    chip.dataset.pending = "true";
    chip.textContent = pendingText;
  } else {
    delete chip.dataset.pending;
    chip.textContent = savedText;
  }
}

// Settings save themselves like the editor does: debounced on input, flushed
// by Ctrl+S, no save button.
const settingsForm = document.querySelector<HTMLFormElement>("[data-settings]");
if (settingsForm) {
  const saveState = document.querySelector<HTMLElement>("[data-save-state]")!;
  const msg = {
    saved: settingsForm.dataset.msgSaved ?? "Saved",
    savedPending: settingsForm.dataset.msgSavedPending ?? "Saved · retrying publication",
    saving: settingsForm.dataset.msgSaving ?? "Saving…",
    unsaved: settingsForm.dataset.msgUnsaved ?? "Unsaved",
  };
  let timer: number | undefined;
  let saving = false;
  let saveAgain = false;
  let dirty = false;

  const save = async (): Promise<void> => {
    if (saving) {
      saveAgain = true;
      return;
    }
    saving = true;
    saveState.textContent = msg.saving;
    try {
      const parameters = new URLSearchParams();
      for (const [name, value] of new FormData(settingsForm)) {
        if (typeof value === "string") parameters.append(name, value);
      }
      const response = await fetch(settingsForm.action, {
        method: "POST",
        headers: { Accept: "application/json" },
        body: parameters,
      });
      if (!response.ok) throw new RequestFailure(response.status, await response.text());
      const result = await response.json();
      if (!saveAgain) {
        dirty = false;
        delete saveState.dataset.error;
        showSaved(saveState, result.site, msg.saved, msg.savedPending);
      }
    } catch (reason) {
      saveState.dataset.error = "true";
      saveState.textContent = describeFailure(reason, failureMessages(settingsForm.dataset));
    } finally {
      saving = false;
      if (saveAgain) {
        saveAgain = false;
        void save();
      }
    }
  };
  const saveNow = (): void => {
    clearTimeout(timer);
    void save();
  };

  settingsForm.addEventListener("input", () => {
    dirty = true;
    saveState.textContent = msg.unsaved;
    clearTimeout(timer);
    timer = window.setTimeout(() => void save(), 1_200);
  });

  // The theme is a whole stylesheet; a bare textarea is no place to edit
  // one. CodeMirror takes over and mirrors back into the form field, so
  // autosave and no-JS submission both keep working unchanged.
  const cssArea = settingsForm.querySelector<HTMLTextAreaElement>('textarea[name="custom_css"]');
  if (cssArea) {
    const cssEditor = new EditorView({
      doc: cssArea.value,
      extensions: [
        basicSetup,
        css(),
        EditorView.lineWrapping,
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            cssArea.value = update.state.doc.toString();
            cssArea.dispatchEvent(new Event("input", { bubbles: true }));
          }
        }),
      ],
    });
    cssArea.before(cssEditor.dom);
    cssArea.hidden = true;
  }
  settingsForm.addEventListener("submit", (event) => {
    event.preventDefault();
    saveNow();
  });
  document.addEventListener("keydown", (event) => {
    if ((event.ctrlKey || event.metaKey) && event.key === "s") {
      event.preventDefault();
      saveNow();
    }
  });
  window.addEventListener("beforeunload", (event) => {
    if (dirty || saving) event.preventDefault();
  });
}

const editor = document.querySelector<HTMLFormElement>("[data-editor]");
if (editor) {
  const csrf = editor.querySelector<HTMLInputElement>("[name=csrf]")!.value;
  const textarea = editor.querySelector<HTMLTextAreaElement>("[data-markdown]")!;
  const titleField = editor.querySelector<HTMLTextAreaElement>("[data-title]")!;
  const slugField = editor.querySelector<HTMLInputElement>("[data-slug]")!;
  const previewSection = editor.querySelector<HTMLElement>("[data-preview]")!;
  const previewFrame = editor.querySelector<HTMLIFrameElement>("[data-preview-frame]")!;
  const previewNote = editor.querySelector<HTMLElement>("[data-preview-note]");
  const previewToggle = editor.querySelector<HTMLButtonElement>("[data-preview-toggle]")!;
  const documentSection = editor.querySelector<HTMLElement>('[data-media-drop="body"]')!;
  const drawer = editor.querySelector<HTMLDialogElement>("[data-drawer]")!;
  const drawerToggle = editor.querySelector<HTMLButtonElement>("[data-drawer-toggle]")!;
  const saveState = editor.querySelector<HTMLElement>("[data-save-state]")!;
  const statusLabel = editor.querySelector<HTMLElement>("[data-status-label]")!;
  const statusTime = editor.querySelector<HTMLElement>("[data-status-time]")!;
  // Publish/Unpublish are absent while the piece sits in the trash.
  const publishButton = editor.querySelector<HTMLButtonElement>("[data-publish]");
  // Unpublish is a two-step <details>: opening it is the confirmation.
  const unpublishButton = editor.querySelector<HTMLDetailsElement>("[data-unpublish]");
  const publishAt = editor.querySelector<HTMLInputElement>("[data-publish-at]")!;
  const publishAtHint = editor.querySelector<HTMLElement>("[data-publish-at-hint]");
  const counter = editor.querySelector<HTMLElement>("[data-count]");
  const shortcuts = editor.querySelector<HTMLElement>("[data-shortcuts]");
  const trashed = editor.dataset.trashed === "true";
  const language = document.documentElement.lang;
  const siteZone = editor.dataset.siteZone ?? "";
  const browserZone = Intl.DateTimeFormat().resolvedOptions().timeZone || "";
  // A scheduled instant in the writer's zone, and in the site's when they differ.
  const describeInstant = (iso: string): string => {
    const instant = new Date(iso);
    let label = instant.toLocaleString(language || undefined);
    if (siteZone && siteZone !== browserZone) {
      try {
        label += ` (${siteZone}: ${instant.toLocaleString(language || undefined, { timeZone: siteZone })})`;
      } catch {
        /* an unknown zone name: the writer's own reading is enough */
      }
    }
    return label;
  };
  const failures = failureMessages(editor.dataset);
  const msg = {
    saved: editor.dataset.msgSaved ?? "Saved",
    savedAt: editor.dataset.msgSavedAt ?? "Saved {time}",
    savedPending: editor.dataset.msgSavedPending ?? "Saved {time} · retrying publication",
    saving: editor.dataset.msgSaving ?? "Saving…",
    unsaved: editor.dataset.msgUnsaved ?? "Unsaved",
    needTitle: editor.dataset.msgNeedTitle ?? "Add a title to start saving",
    statusDraft: editor.dataset.msgStatusDraft ?? "Draft",
    statusScheduled: editor.dataset.msgStatusScheduled ?? "Scheduled",
    statusPublic: editor.dataset.msgStatusPublic ?? "Public",
    publish: editor.dataset.msgPublish ?? "Publish",
    schedule: editor.dataset.msgSchedule ?? "Schedule",
    publishAtHint: editor.dataset.msgPublishAtHint ?? "",
    count: editor.dataset.msgCount ?? "{chars} characters · {words} words",
    shortcuts: editor.dataset.msgShortcuts ?? "",
    slugInvalid: editor.dataset.msgSlugInvalid ?? "The slug may only use lowercase letters, digits, and hyphens.",
    shareCopied: editor.dataset.msgShareCopied ?? "Copied",
    shareExpires: editor.dataset.msgShareExpires ?? "Valid until {time}",
  };
  let autosaveTimer: number | undefined;
  let saving = false;
  let saveAgain = false;
  let dirty = false;
  let pendingStatus: "public" | "draft" | undefined;

  // The server stores UTC; the control shows the writer's own zone.
  if (publishAt.dataset.publishAtUtc) {
    publishAt.value = isoToLocalDateTime(publishAt.dataset.publishAtUtc);
  }
  publishAt.dataset.initial = publishAt.value;
  if (publishAtHint && msg.publishAtHint) {
    const zone = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
    publishAtHint.textContent = msg.publishAtHint.replace("{zone}", zone);
  }
  const chosenInstantIsFuture = (): boolean => {
    const iso = localDateTimeToIso(publishAt.value);
    return iso !== null && new Date(iso).getTime() > Date.now();
  };
  const refreshPublishLabel = (): void => {
    if (!publishButton || publishButton.hidden) return;
    publishButton.textContent = chosenInstantIsFuture() ? msg.schedule : msg.publish;
  };
  publishAt.addEventListener("input", refreshPublishLabel);

  // Mirrors the server's answer: the status chip, the scheduled instant, and
  // which of Publish/Unpublish is on offer.
  const applyStatus = (status: string, publishAtUtc: string | null | undefined): void => {
    const labels: Record<string, string> = {
      draft: msg.statusDraft,
      scheduled: msg.statusScheduled,
      public: msg.statusPublic,
    };
    statusLabel.textContent = labels[status] ?? status;
    statusTime.hidden = status !== "scheduled";
    if (publishAtUtc) {
      statusTime.setAttribute("datetime", publishAtUtc);
      statusTime.textContent = describeInstant(publishAtUtc);
      publishAt.value = isoToLocalDateTime(publishAtUtc);
      publishAt.dataset.publishAtUtc = publishAtUtc;
    } else {
      publishAt.dataset.publishAtUtc = "";
    }
    publishAt.dataset.initial = publishAt.value;
    if (publishButton) publishButton.hidden = status === "public";
    if (unpublishButton) unpublishButton.hidden = status === "draft";
    refreshPublishLabel();
  };
  if (statusTime.getAttribute("datetime")) {
    statusTime.textContent = describeInstant(statusTime.getAttribute("datetime")!);
  }

  // Mod-K wraps the selection as a markdown link, or drops in a template.
  const insertLink = (view: EditorView): boolean => {
    const selection = view.state.selection.main;
    const selected = view.state.sliceDoc(selection.from, selection.to);
    if (selected) {
      const insertion = `[${selected}]()`;
      view.dispatch({
        changes: { from: selection.from, to: selection.to, insert: insertion },
        selection: { anchor: selection.from + insertion.length - 1 },
      });
    } else {
      view.dispatch({
        changes: { from: selection.from, to: selection.to, insert: "[text](url)" },
        selection: { anchor: selection.from + 1, head: selection.from + 5 },
      });
    }
    return true;
  };

  // minimalSetup on purpose: basicSetup's autocompletion offers HTML tags
  // that this pipeline never renders, and prose needs none of the rest.
  const codeEditor = new EditorView({
    doc: textarea.value,
    extensions: [
      keymap.of([
        { key: "Mod-k", run: insertLink },
        {
          key: "Mod-Shift-p",
          run: () => {
            previewToggle.click();
            return true;
          },
        },
      ]),
      segmentKeymap,
      minimalSetup,
      markdown(),
      EditorView.lineWrapping,
      EditorState.readOnly.of(trashed),
      EditorView.editable.of(!trashed),
      EditorView.updateListener.of((update) => {
        if (update.docChanged) {
          textarea.value = update.state.doc.toString();
          textarea.dispatchEvent(new Event("input", { bubbles: true }));
          scheduleCount();
        }
      }),
    ],
  });
  textarea.before(codeEditor.dom);
  textarea.hidden = true;
  textarea.required = false;

  // Characters and words, refreshed at most once per frame.
  let countFrame = 0;
  const refreshCount = (): void => {
    countFrame = 0;
    if (!counter) return;
    const { chars, words } = countText(codeEditor.state.doc.toString(), wordSegmenter);
    counter.textContent = msg.count
      .replace("{chars}", String(chars))
      .replace("{words}", String(words));
    counter.hidden = false;
  };
  const scheduleCount = (): void => {
    if (countFrame === 0) countFrame = requestAnimationFrame(refreshCount);
  };
  refreshCount();

  if (shortcuts && msg.shortcuts) {
    const platform = navigator.platform || navigator.userAgent;
    const mod = /Mac|iPhone|iPad/.test(platform) ? "⌘" : "Ctrl";
    shortcuts.textContent = msg.shortcuts.replaceAll("{mod}", mod);
    shortcuts.hidden = false;
  }

  // The title looks like the first line of the document: it grows with its
  // content, and Enter or ArrowDown continues into the body.
  const resizeTitle = (): void => {
    titleField.style.height = "auto";
    titleField.style.height = `${titleField.scrollHeight}px`;
  };
  resizeTitle();
  titleField.addEventListener("input", resizeTitle);
  titleField.addEventListener("keydown", (event) => {
    if (event.key === "Enter" || event.key === "ArrowDown") {
      event.preventDefault();
      codeEditor.focus();
    }
  });

  // What was typed since the last autosave also lives in this browser, so a
  // closed tab, an expired session or a failed save never costs the text.
  const localStore = createDraftStore(localStorage, draftKey(editor.dataset.contentId || null));
  const summaryField = editor.querySelector<HTMLTextAreaElement>('[name="summary"]');
  const tagsField = editor.querySelector<HTMLInputElement>('[name="tags"]');
  const draftBar = editor.querySelector<HTMLElement>("[data-local-draft]");
  const draftText = editor.querySelector<HTMLElement>("[data-local-draft-text]");
  const snapshotDraft = (): LocalDraft => ({
    title: titleField.value,
    body: codeEditor.state.doc.toString(),
    slug: slugField.value,
    summary: summaryField?.value ?? "",
    tags: tagsField?.value ?? "",
    savedAt: new Date().toISOString(),
    version: Number(editor.querySelector<HTMLInputElement>("[name=version]")?.value ?? "") || null,
  });
  let draftTimer: number | undefined;
  const rememberDraft = (): void => {
    clearTimeout(draftTimer);
    draftTimer = window.setTimeout(() => localStore.write(snapshotDraft()), 500);
  };
  const restoreDraft = (local: LocalDraft): void => {
    titleField.value = local.title;
    if (summaryField) summaryField.value = local.summary;
    if (tagsField) tagsField.value = local.tags;
    const previousSlug = slugField.value;
    slugField.value = local.slug;
    if (!slugField.checkValidity()) slugField.value = previousSlug;
    codeEditor.dispatch({
      changes: { from: 0, to: codeEditor.state.doc.length, insert: local.body },
    });
    resizeTitle();
    editor.dispatchEvent(new Event("input", { bubbles: true }));
  };
  const offered = localStore.read();
  if (
    offered &&
    draftBar &&
    draftText &&
    shouldOfferRestore(offered, {
      updatedAt: editor.dataset.updatedAt ?? "",
      body: textarea.value,
      trashed,
    })
  ) {
    draftText.textContent = (
      editor.dataset.msgLocalDraft ?? "This browser has unsaved changes from {time}."
    ).replace("{time}", formatLocalDateTime(new Date(offered.savedAt), language));
    draftBar.hidden = false;
    editor
      .querySelector<HTMLButtonElement>("[data-local-draft-restore]")
      ?.addEventListener("click", () => {
        restoreDraft(offered);
        draftBar.hidden = true;
      });
    editor
      .querySelector<HTMLButtonElement>("[data-local-draft-discard]")
      ?.addEventListener("click", () => {
        localStore.clear();
        draftBar.hidden = true;
      });
  }

  const formParameters = (): URLSearchParams => {
    const parameters = new URLSearchParams();
    for (const [name, value] of new FormData(editor)) {
      if (typeof value === "string") parameters.append(name, value);
    }
    parameters.set("intent", "autosave");
    // The control holds a local time; the server takes an instant. It is
    // sent only when the writer changed it or is publishing, so an untouched
    // date never re-dates a piece.
    if (publishAt.value !== publishAt.dataset.initial || pendingStatus) {
      parameters.set("publish_at", localDateTimeToIso(publishAt.value) ?? "");
    } else {
      parameters.delete("publish_at");
    }
    return parameters;
  };

  const autosave = async (): Promise<void> => {
    if (saving) {
      saveAgain = true;
      return;
    }
    if (titleField.value.trim() === "") {
      // Dropping the queued publish keeps a later unrelated save from
      // publishing as a surprise side effect.
      pendingStatus = undefined;
      saveState.textContent = msg.needTitle;
      return;
    }
    if (!slugField.checkValidity()) {
      pendingStatus = undefined;
      saveState.dataset.error = "true";
      saveState.textContent = msg.slugInvalid;
      return;
    }
    saving = true;
    delete saveState.dataset.error;
    saveState.textContent = msg.saving;
    const statusToSend = pendingStatus;
    try {
      const parameters = formParameters();
      if (statusToSend) {
        parameters.set("status", statusToSend);
        parameters.set("intent", "explicit");
      }
      const response = await fetch(editor.action, {
        method: "POST",
        headers: { Accept: "application/json" },
        body: parameters,
      });
      if (isConflictPage(response.status, response.headers.get("content-type"))) {
        // Let the browser render the server's full conflict page as a normal
        // navigation. This preserves every submitted field without evaluating
        // response text through document.write. Other 409s (a taken slug)
        // fall through to the ordinary error path and keep the editor intact.
        dirty = false;
        saving = false;
        saveAgain = false;
        pendingStatus = undefined;
        postFormAsNavigation(
          document,
          editor.action,
          conflictNavigationParameters(parameters),
        );
        return;
      }
      if (!response.ok) throw new RequestFailure(response.status, await response.text());
      const result = await response.json();
      let version = editor.querySelector<HTMLInputElement>("[name=version]");
      if (!version) {
        version = document.createElement("input");
        version.type = "hidden";
        version.name = "version";
        editor.append(version);
        editor.action = `/admin/content/${result.id}/`;
        history.replaceState(null, "", `/admin/content/${result.id}/edit/`);
        previewFrame.dataset.previewUrl = `/admin/content/${result.id}/preview/`;
        localStore.moveTo(draftKey(result.id));
      }
      version.value = String(result.version);
      const trashVersion = document.querySelector<HTMLInputElement>("[data-trash-version]");
      if (trashVersion) trashVersion.value = String(result.version);
      if (result.slug && slugField.value.trim() === "") {
        slugField.value = result.slug;
      }
      if (statusToSend) pendingStatus = undefined;
      if (typeof result.status === "string") {
        applyStatus(result.status, result.publish_at);
      }
      if (!saveAgain) {
        dirty = false;
        // Everything typed so far is on the server now; an edit that arrived
        // mid-save keeps its local copy until its own save lands.
        clearTimeout(draftTimer);
        localStore.clear();
        const time = formatLocalTime(new Date(), language);
        showSaved(
          saveState,
          result.site,
          msg.savedAt.replace("{time}", time),
          msg.savedPending.replace("{time}", time),
        );
      }
      schedulePreviewReload();
    } catch (reason) {
      saveState.dataset.error = "true";
      saveState.textContent = describeFailure(reason, failures);
    } finally {
      saving = false;
      // Re-run for a queued follow-up, or for a publish/unpublish that was
      // clicked while this save was in flight (never for one that just failed,
      // which would retry forever).
      if (saveAgain || (pendingStatus && !statusToSend)) {
        saveAgain = false;
        void autosave();
      }
    }
  };

  const saveNow = (): void => {
    clearTimeout(autosaveTimer);
    void autosave();
  };

  // A trashed piece is read-only: nothing autosaves and nothing publishes
  // until it is restored through its own form.
  if (!trashed) {
    editor.addEventListener("input", () => {
      dirty = true;
      saveState.textContent = msg.unsaved;
      rememberDraft();
      clearTimeout(autosaveTimer);
      autosaveTimer = window.setTimeout(() => void autosave(), 1_200);
    });
    editor.addEventListener("submit", (event) => {
      event.preventDefault();
      // The browser's own constraint messages point at the offending field.
      if (!editor.reportValidity()) return;
      const submitter = (event as SubmitEvent).submitter;
      if (
        submitter instanceof HTMLButtonElement &&
        submitter.name === "status" &&
        (submitter.value === "public" || submitter.value === "draft")
      ) {
        pendingStatus = submitter.value;
        if (unpublishButton) unpublishButton.open = false;
      }
      saveNow();
    });
    document.addEventListener("keydown", (event) => {
      if ((event.ctrlKey || event.metaKey) && !event.shiftKey && event.key === "s") {
        event.preventDefault();
        saveNow();
      }
      if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === "p") {
        event.preventDefault();
        previewToggle.click();
      }
    });
    window.addEventListener("beforeunload", (event) => {
      if (dirty || saving) event.preventDefault();
    });
  }

  // The preview is the real page in a frame: beside the text on a wide
  // screen, in place of it on a narrow one, reloaded after every save.
  const wide = matchMedia("(min-width: 1100px)");
  let previewOpen = false;
  let previewTimer: number | undefined;
  const loadPreview = (): void => {
    const url = previewFrame.dataset.previewUrl;
    if (!url) {
      if (previewNote) previewNote.hidden = false;
      return;
    }
    if (previewNote) previewNote.hidden = true;
    if (previewFrame.getAttribute("src") === url) {
      previewFrame.contentWindow?.location.reload();
    } else {
      previewFrame.src = url;
    }
  };
  const layoutPreview = (): void => {
    previewSection.hidden = !previewOpen;
    documentSection.hidden = previewOpen && !wide.matches;
    editor.dataset.previewOpen = String(previewOpen);
    previewToggle.setAttribute("aria-pressed", String(previewOpen));
  };
  const schedulePreviewReload = (): void => {
    if (!previewOpen) return;
    clearTimeout(previewTimer);
    previewTimer = window.setTimeout(loadPreview, 600);
  };
  previewToggle.addEventListener("click", () => {
    previewOpen = !previewOpen;
    layoutPreview();
    if (previewOpen) loadPreview();
  });
  wide.addEventListener("change", layoutPreview);

  // Share links: the form still posts on its own without scripting; with it,
  // the link lands in the drawer with a copy button.
  const shareForm = document.querySelector<HTMLFormElement>("[data-share-form]");
  const shareResult = editor.querySelector<HTMLElement>("[data-share-result]");
  const shareUrl = editor.querySelector<HTMLOutputElement>("[data-share-url]");
  const shareExpires = editor.querySelector<HTMLElement>("[data-share-expires]");
  const shareCopy = editor.querySelector<HTMLButtonElement>("[data-share-copy]");
  shareForm?.addEventListener("submit", async (event) => {
    event.preventDefault();
    try {
      const response = await fetch(shareForm.action, {
        method: "POST",
        headers: { Accept: "application/json" },
        body: new URLSearchParams(new FormData(shareForm) as unknown as Record<string, string>),
      });
      if (!response.ok) throw new RequestFailure(response.status, await response.text());
      const link = await response.json();
      if (shareUrl) shareUrl.value = `${location.origin}${link.url}`;
      if (shareExpires) {
        shareExpires.textContent = msg.shareExpires.replace(
          "{time}",
          formatLocalDateTime(new Date(link.expires_at), language),
        );
      }
      if (shareResult) shareResult.hidden = false;
      if (shareCopy) shareCopy.textContent = editor.dataset.msgShareCopy ?? shareCopy.textContent;
    } catch (reason) {
      saveState.dataset.error = "true";
      saveState.textContent = describeFailure(reason, failures);
    }
  });
  shareCopy?.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(shareUrl?.value ?? "");
      shareCopy.textContent = msg.shareCopied;
    } catch {
      shareUrl?.focus();
    }
  });

  const setDrawer = (open: boolean): void => {
    if (open && !drawer.open) drawer.showModal();
    else if (!open && drawer.open) drawer.close();
    drawerToggle.setAttribute("aria-expanded", String(open));
  };
  drawerToggle.addEventListener("click", () => setDrawer(!drawer.open));
  drawer.addEventListener("close", () => drawerToggle.setAttribute("aria-expanded", "false"));
  // A click on the backdrop lands on the dialog itself, outside its box.
  drawer.addEventListener("click", (event) => {
    if (event.target !== drawer) return;
    const box = drawer.getBoundingClientRect();
    const inside =
      event.clientX >= box.left &&
      event.clientX <= box.right &&
      event.clientY >= box.top &&
      event.clientY <= box.bottom;
    if (!inside) setDrawer(false);
  });

  // Dropping into the document body uploads and inserts markdown in place.
  if (!trashed) wireDropZone(documentSection, async (file) => {
    const media = await uploadMedia(csrf, file);
    const selection = codeEditor.state.selection.main;
    const insertion = `![${media.alt_text || file.name}](${media.url})`;
    codeEditor.dispatch({
      changes: { from: selection.from, to: selection.to, insert: insertion },
      selection: { anchor: selection.from + insertion.length },
    });
    editor.dispatchEvent(new Event("input", { bubbles: true }));
  });
}
