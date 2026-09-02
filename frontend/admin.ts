import { EditorView, basicSetup, minimalSetup } from "codemirror";
import { keymap } from "@codemirror/view";
import { EditorSelection, type EditorState } from "@codemirror/state";
import { markdown } from "@codemirror/lang-markdown";
import { css } from "@codemirror/lang-css";
import { postFormAsNavigation } from "./form-navigation";

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
    throw new Error(payload.error || "Request failed");
  }
  return payload;
}

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : "Request failed";
}

async function uploadMedia(csrf: string, file: File): Promise<any> {
  const data = new FormData();
  data.set("csrf", csrf);
  data.set("alt_text", file.name);
  data.set("file", file);
  const response = await fetch("/admin/media/", { method: "POST", body: data });
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}

function wireDropZone(zone: HTMLElement, onFile: (file: File) => Promise<void>): void {
  const handle = async (file: File | undefined): Promise<void> => {
    if (!file) return;
    try {
      await onFile(file);
    } catch (reason) {
      const hint = zone.querySelector<HTMLElement>("small");
      if (hint) hint.textContent = errorMessage(reason);
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
  clear.addEventListener("click", () => {
    input.value = "";
    thumb.hidden = true;
    thumb.removeAttribute("src");
    clear.hidden = true;
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  wireDropZone(zone, async (file) => {
    const media = await uploadMedia(csrfMeta, file);
    input.value = media.id;
    thumb.src = media.url;
    thumb.hidden = false;
    clear.hidden = false;
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
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
    location.href = "/admin/";
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

// Settings save themselves like the editor does: debounced on input, flushed
// by Ctrl+S, no save button.
const settingsForm = document.querySelector<HTMLFormElement>("[data-settings]");
if (settingsForm) {
  const saveState = document.querySelector<HTMLElement>("[data-save-state]")!;
  const msg = {
    saved: settingsForm.dataset.msgSaved ?? "Saved",
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
      if (!response.ok) throw new Error(await response.text());
      if (!saveAgain) {
        dirty = false;
        saveState.textContent = msg.saved;
      }
    } catch (reason) {
      saveState.textContent = errorMessage(reason);
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
  const preview = editor.querySelector<HTMLElement>("[data-preview-output]")!;
  const previewSection = editor.querySelector<HTMLElement>("[data-preview]")!;
  const previewToggle = editor.querySelector<HTMLButtonElement>("[data-preview-toggle]")!;
  const documentSection = editor.querySelector<HTMLElement>('[data-media-drop="body"]')!;
  const drawer = editor.querySelector<HTMLElement>("[data-drawer]")!;
  const drawerToggle = editor.querySelector<HTMLButtonElement>("[data-drawer-toggle]")!;
  const drawerBackdrop = editor.querySelector<HTMLElement>("[data-drawer-backdrop]")!;
  const saveState = editor.querySelector<HTMLElement>("[data-save-state]")!;
  const statusLabel = editor.querySelector<HTMLElement>("[data-status-label]")!;
  const publishButton = editor.querySelector<HTMLButtonElement>("[data-publish]")!;
  const unpublishButton = editor.querySelector<HTMLButtonElement>("[data-unpublish]")!;
  const msg = {
    saved: editor.dataset.msgSaved ?? "Saved",
    saving: editor.dataset.msgSaving ?? "Saving…",
    unsaved: editor.dataset.msgUnsaved ?? "Unsaved",
    needTitle: editor.dataset.msgNeedTitle ?? "Add a title to start saving",
    statusDraft: editor.dataset.msgStatusDraft ?? "Draft",
    statusPublic: editor.dataset.msgStatusPublic ?? "Public",
  };
  let autosaveTimer: number | undefined;
  let saving = false;
  let saveAgain = false;
  let dirty = false;
  let pendingStatus: "public" | "draft" | undefined;

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
      keymap.of([{ key: "Mod-k", run: insertLink }]),
      segmentKeymap,
      minimalSetup,
      markdown(),
      EditorView.lineWrapping,
      EditorView.updateListener.of((update) => {
        if (update.docChanged) {
          textarea.value = update.state.doc.toString();
          textarea.dispatchEvent(new Event("input", { bubbles: true }));
        }
      }),
    ],
  });
  textarea.before(codeEditor.dom);
  textarea.hidden = true;
  textarea.required = false;

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

  const formParameters = (): URLSearchParams => {
    const parameters = new URLSearchParams();
    for (const [name, value] of new FormData(editor)) {
      if (typeof value === "string") parameters.append(name, value);
    }
    parameters.set("intent", "autosave");
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
    saving = true;
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
      if (response.status === 409) {
        // Let the browser render the server's full conflict page as a normal
        // navigation. This preserves every submitted field without evaluating
        // response text through document.write.
        dirty = false;
        saving = false;
        saveAgain = false;
        pendingStatus = undefined;
        postFormAsNavigation(document, editor.action, parameters);
        return;
      }
      if (!response.ok) throw new Error(await response.text());
      const result = await response.json();
      let version = editor.querySelector<HTMLInputElement>("[name=version]");
      if (!version) {
        version = document.createElement("input");
        version.type = "hidden";
        version.name = "version";
        editor.append(version);
        editor.action = `/admin/content/${result.id}/`;
        history.replaceState(null, "", `/admin/content/${result.id}/edit/`);
      }
      version.value = String(result.version);
      if (result.slug && slugField.value.trim() === "") {
        slugField.value = result.slug;
      }
      if (statusToSend) {
        pendingStatus = undefined;
        const isPublic = statusToSend === "public";
        publishButton.hidden = isPublic;
        unpublishButton.hidden = !isPublic;
        statusLabel.textContent = isPublic ? msg.statusPublic : msg.statusDraft;
      }
      if (!saveAgain) {
        dirty = false;
        saveState.textContent = msg.saved;
      }
    } catch (reason) {
      saveState.textContent = errorMessage(reason);
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

  editor.addEventListener("input", () => {
    dirty = true;
    saveState.textContent = msg.unsaved;
    clearTimeout(autosaveTimer);
    autosaveTimer = window.setTimeout(() => void autosave(), 1_200);
  });
  editor.addEventListener("submit", (event) => {
    event.preventDefault();
    const submitter = (event as SubmitEvent).submitter;
    if (
      submitter instanceof HTMLButtonElement &&
      submitter.name === "status" &&
      (submitter.value === "public" || submitter.value === "draft")
    ) {
      pendingStatus = submitter.value;
    }
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

  previewToggle.addEventListener("click", async () => {
    const showing = !previewSection.hidden;
    if (showing) {
      previewSection.hidden = true;
      documentSection.hidden = false;
      previewToggle.setAttribute("aria-pressed", "false");
      return;
    }
    try {
      const result = await post("/admin/preview/", { csrf, markdown: textarea.value });
      preview.innerHTML = result.html;
    } catch (reason) {
      preview.textContent = errorMessage(reason);
    }
    documentSection.hidden = true;
    previewSection.hidden = false;
    previewToggle.setAttribute("aria-pressed", "true");
  });

  const setDrawer = (open: boolean): void => {
    drawer.hidden = !open;
    drawerBackdrop.hidden = !open;
    drawerToggle.setAttribute("aria-expanded", String(open));
  };
  drawerToggle.addEventListener("click", () => setDrawer(drawer.hidden));
  drawerBackdrop.addEventListener("click", () => setDrawer(false));
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !drawer.hidden) setDrawer(false);
  });

  // Dropping into the document body uploads and inserts markdown in place.
  wireDropZone(documentSection, async (file) => {
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
