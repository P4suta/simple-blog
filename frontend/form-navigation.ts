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
