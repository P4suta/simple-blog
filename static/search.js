// Static public search. The generated index contains only content visible in
// the active release; no query or reader data leaves the browser.
(() => {
  "use strict";

  const fold = (value) =>
    value
      .normalize("NFKC")
      .replace(/[A-Z]/gu, (character) => character.toLowerCase())
      .replace(/[ァ-ヶ]/gu, (character) =>
        String.fromCodePoint(character.codePointAt(0) - 0x60),
      );
  const terms = (value) => [
    ...new Set(
      [...fold(value)]
        .slice(0, 120)
        .join("")
        .trim()
        .split(/\s+/u)
        .filter(Boolean),
    ),
  ].slice(0, 8);

  const queryDocuments = (documents, query, limit = 50) => {
    const wanted = terms(query);
    if (wanted.length === 0 || limit <= 0) return [];
    return documents
      .map((document, sourcePosition) => {
        const title = fold(document.title || "");
        const summary = fold(document.summary || "");
        const body = fold(document.body || "");
        const score = wanted.reduce(
          (total, term) => total
            + (title.includes(term) ? 100 : 0)
            + (summary.includes(term) ? 10 : 0)
            + (body.includes(term) ? 1 : 0),
          0,
        );
        return { document, sourcePosition, score };
      })
      .filter(({ document }) =>
        wanted.every((term) => document.folded.includes(term)))
      .sort((left, right) =>
        right.score - left.score || left.sourcePosition - right.sourcePosition)
      .slice(0, limit)
      .map(({ document }) => document);
  };

  if (typeof module === "object" && module.exports) {
    module.exports = { fold, queryDocuments, terms };
  }
  if (typeof document === "undefined") return;

  const form = document.querySelector("form[data-static-search]");
  const results = document.querySelector("[data-search-results]");
  const status = document.querySelector("[data-search-status]");
  if (!form || !results || !status) return;

  const render = (documents, query) => {
    results.replaceChildren();
    const wanted = terms(query);
    if (wanted.length === 0) {
      status.textContent = "";
      results.hidden = true;
      return;
    }
    const matches = queryDocuments(documents, query, 50);
    status.textContent = matches.length === 0
      ? status.dataset.empty || "Nothing found."
      : "";
    for (const document of matches) {
      // `document` above is a search record, so use the global explicitly.
      const node = globalThis.document.createElement("article");
      const time = globalThis.document.createElement("time");
      time.textContent = document.published;
      const heading = globalThis.document.createElement("h2");
      const link = globalThis.document.createElement("a");
      link.href = `/${encodeURIComponent(document.slug)}/`;
      link.textContent = document.title;
      heading.append(link);
      const summary = globalThis.document.createElement("p");
      summary.textContent = document.summary || document.body.slice(0, 180);
      node.append(time, heading, summary);
      results.append(node);
    }
    results.hidden = false;
  };

  const query = new URL(location.href).searchParams.get("q") || "";
  const input = form.elements.namedItem("q");
  if (input) input.value = query;
  if (!query.trim()) return;
  status.textContent = status.dataset.loading || "Searching…";
  // The index URL carries the release's fingerprint (immutable caching
  // would otherwise pin a returning reader to a year-old corpus).
  const indexUrl = form.dataset.index || "/assets/search-index.json";
  fetch(indexUrl, { headers: { accept: "application/json" } })
    .then((response) => {
      if (!response.ok) throw new Error(String(response.status));
      return response.json();
    })
    .then((index) => render(index.documents || [], query))
    .catch(() => {
      status.textContent = status.dataset.failed || "Search is temporarily unavailable.";
    });
})();
