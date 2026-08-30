import { EditorView, basicSetup } from "codemirror";
import { markdown } from "@codemirror/lang-markdown";

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

function showRecoveryCodes(codes: string[]): void {
  const main = document.createElement("main");
  main.className = "auth-card";
  const heading = document.createElement("h1");
  heading.textContent = "Recovery codes";
  const explanation = document.createElement("p");
  explanation.textContent = "一度だけ表示されます。安全な場所へ保存してください。";
  const output = document.createElement("pre");
  output.textContent = codes.join("\n");
  const link = document.createElement("a");
  link.className = "primary-button";
  link.href = "/admin/";
  link.textContent = "管理画面へ";
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
    showRecoveryCodes(completed.recovery_codes);
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

const editor = document.querySelector<HTMLFormElement>("[data-editor]");
if (editor) {
  const csrf = editor.querySelector<HTMLInputElement>("[name=csrf]")!.value;
  const textarea = editor.querySelector<HTMLTextAreaElement>("[data-markdown]")!;
  const preview = editor.querySelector<HTMLElement>("[data-preview-output]")!;
  const saveState = editor.querySelector<HTMLElement>("[data-save-state]")!;
  let previewTimer: number | undefined;
  let autosaveTimer: number | undefined;
  let saving = false;
  let saveAgain = false;

  const codeEditor = new EditorView({
    doc: textarea.value,
    extensions: [
      basicSetup,
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

  const updatePreview = async (): Promise<void> => {
    try {
      const result = await post("/admin/preview/", {
        csrf,
        markdown: textarea.value,
      });
      preview.innerHTML = result.html;
    } catch (reason) {
      preview.textContent = errorMessage(reason);
    }
  };

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
    saving = true;
    saveState.textContent = "保存中…";
    try {
      const response = await fetch(editor.action, {
        method: "POST",
        headers: { Accept: "application/json" },
        body: formParameters(),
      });
      if (response.status === 409) {
        document.open();
        document.write(await response.text());
        document.close();
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
      }
      version.value = String(result.version);
      saveState.textContent = "保存済み";
    } catch (reason) {
      saveState.textContent = errorMessage(reason);
    } finally {
      saving = false;
      if (saveAgain) {
        saveAgain = false;
        void autosave();
      }
    }
  };

  editor.addEventListener("input", () => {
    saveState.textContent = "未保存";
    clearTimeout(autosaveTimer);
    autosaveTimer = window.setTimeout(() => void autosave(), 1_200);
  });
  textarea.addEventListener("input", () => {
    clearTimeout(previewTimer);
    previewTimer = window.setTimeout(() => void updatePreview(), 250);
  });
  editor.addEventListener("submit", () => {
    textarea.value = codeEditor.state.doc.toString();
  });
  void updatePreview();

  const upload = async (file: File): Promise<any> => {
    const data = new FormData();
    data.set("csrf", csrf);
    data.set("alt_text", file.name);
    data.set("file", file);
    const response = await fetch("/admin/media/", { method: "POST", body: data });
    if (!response.ok) throw new Error(await response.text());
    return response.json();
  };

  editor.querySelectorAll<HTMLElement>("[data-media-drop]").forEach((zone) => {
    zone.addEventListener("dragover", (event) => {
      event.preventDefault();
      zone.dataset.dragging = "true";
    });
    zone.addEventListener("dragleave", () => delete zone.dataset.dragging);
    zone.addEventListener("drop", async (event) => {
      event.preventDefault();
      delete zone.dataset.dragging;
      const file = event.dataTransfer?.files[0];
      if (!file) return;
      try {
        const media = await upload(file);
        if (zone.dataset.mediaDrop === "cover") {
          editor.querySelector<HTMLInputElement>("[name=cover_media_id]")!.value = media.id;
        } else {
          const selection = codeEditor.state.selection.main;
          const insertion = `![${media.alt_text || file.name}](${media.url})`;
          codeEditor.dispatch({
            changes: { from: selection.from, to: selection.to, insert: insertion },
            selection: { anchor: selection.from + insertion.length },
          });
        }
        editor.dispatchEvent(new Event("input", { bubbles: true }));
      } catch (reason) {
        const hint = zone.querySelector<HTMLElement>("small");
        if (hint) hint.textContent = errorMessage(reason);
      }
    });
  });
}
