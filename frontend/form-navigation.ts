/**
 * Replays an already validated URL-encoded request as a browser navigation.
 * The browser renders the server response as a new document, so callers never
 * need to evaluate response text with document.write or an innerHTML sink.
 */
export function postFormAsNavigation(
  targetDocument: Document,
  action: string,
  parameters: URLSearchParams,
): void {
  const form = targetDocument.createElement("form");
  form.action = action;
  form.method = "post";
  form.hidden = true;
  for (const [name, value] of parameters) {
    const input = targetDocument.createElement("input");
    input.type = "hidden";
    input.name = name;
    input.value = value;
    form.append(input);
  }
  targetDocument.body.append(form);
  // A successful control named "submit" shadows form.submit through the
  // platform's named-property lookup. Calling the prototype remains reliable
  // even if a future editor field uses that name.
  HTMLFormElement.prototype.submit.call(form);
}

/**
 * Only the server's own conflict page may replace the editor. Any other 409
 * (a taken slug, for instance) is an ordinary validation failure that must
 * stay inside the editor with the writer's text intact.
 */
export function isConflictPage(status: number, contentType: string | null): boolean {
  return status === 409 && (contentType ?? "").toLowerCase().includes("text/html");
}

/**
 * The replayed request becomes a top-level navigation, so it must ask for the
 * HTML outcome of an explicit save: a conflict renders the comparison page,
 * and a success redirects back to the editor instead of showing raw JSON.
 */
export function conflictNavigationParameters(parameters: URLSearchParams): URLSearchParams {
  const replay = new URLSearchParams(parameters);
  replay.set("intent", "explicit");
  return replay;
}
