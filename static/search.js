// Static public search. The generated index contains only content visible in
// the active release; no query or reader data leaves the browser. Results
// appear as the reader types, with the matched words marked in titles and
// snippets. Scoring mirrors the Rust oracle in src/application/static_search.rs
// and is tested against it by frontend/search.test.cjs.
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

  // Folding changes lengths (a full-width letter is one source character
  // and one folded one, but ㍿ becomes four), so each source character is
  // folded on its own and every folded UTF-16 unit remembers where it came
  // from. Folding characters separately can differ from folding the whole
  // string when a combining mark follows a base (か + ゛), which only costs a
  // highlight: filtering still uses the whole-string fold from the index.
  const foldMap = (text) => {
    const source = [...text];
    let folded = "";
    const origin = [];
    source.forEach((character, index) => {
      const piece = fold(character);
      folded += piece;
      for (let unit = 0; unit < piece.length; unit += 1) origin.push(index);
    });
    return { source, folded, origin };
  };

  // [start, end) ranges over the source characters that any term matches,
  // merged where they touch or overlap.
  const matchRanges = (text, wanted) => {
    const { folded, origin, source } = foldMap(text);
    const ranges = [];
    for (const term of wanted) {
      if (!term) continue;
      let from = folded.indexOf(term);
      while (from !== -1) {
        const last = from + term.length - 1;
        ranges.push([origin[from], origin[last] + 1]);
        from = folded.indexOf(term, from + 1);
      }
    }
    ranges.sort((left, right) => left[0] - right[0] || right[1] - left[1]);
    const merged = [];
    for (const range of ranges) {
      const previous = merged[merged.length - 1];
      if (previous && range[0] <= previous[1]) previous[1] = Math.max(previous[1], range[1]);
      else merged.push([range[0], range[1]]);
    }
    return { merged, source };
  };

  // Segments covering all of `text`, hit segments being what a term matched.
  const highlight = (text, wanted) => {
    const { merged, source } = matchRanges(text, wanted);
    const segments = [];
    let at = 0;
    for (const [start, end] of merged) {
      if (start > at) segments.push({ text: source.slice(at, start).join(""), hit: false });
      segments.push({ text: source.slice(start, end).join(""), hit: true });
      at = end;
    }
    if (at < source.length) segments.push({ text: source.slice(at).join(""), hit: false });
    if (segments.length === 0) segments.push({ text: "", hit: false });
    return segments;
  };

  // A window of `size` source characters around the first match, or the
  // opening of the text when nothing matches; "…" marks where it was cut.
  const snippet = (body, wanted, size = 120) => {
    const { merged, source } = matchRanges(body, wanted);
    const first = merged.length > 0 ? merged[0][0] : 0;
    let start = merged.length > 0 ? Math.max(0, first - Math.floor(size / 3)) : 0;
    const end = Math.min(source.length, start + size);
    start = Math.min(start, Math.max(0, end - size));
    const window = source.slice(start, end).join("");
    const segments = highlight(window, wanted);
    if (start > 0) segments.unshift({ text: "…", hit: false });
    if (end < source.length) segments.push({ text: "…", hit: false });
    return segments;
  };

  if (typeof module === "object" && module.exports) {
    module.exports = { fold, foldMap, highlight, matchRanges, queryDocuments, snippet, terms };
  }
  if (typeof document === "undefined") return;

  const form = document.querySelector("form[data-static-search]");
  const results = document.querySelector("[data-search-results]");
  const status = document.querySelector("[data-search-status]");
  if (!form || !results || !status) return;
  const input = form.elements.namedItem("q");
  if (!input) return;

  const renderSegments = (parent, segments) => {
    for (const segment of segments) {
      if (segment.hit) {
        const mark = document.createElement("mark");
        mark.textContent = segment.text;
        parent.append(mark);
      } else {
        parent.append(document.createTextNode(segment.text));
      }
    }
  };

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
      renderSegments(link, highlight(document.title, wanted));
      heading.append(link);
      const summary = globalThis.document.createElement("p");
      renderSegments(summary, snippet(document.summary || document.body || "", wanted));
      node.append(time, heading, summary);
      results.append(node);
    }
    results.hidden = false;
  };

  // The index URL carries the release's fingerprint (immutable caching
  // would otherwise pin a returning reader to a year-old corpus). It is
  // fetched once, the first time it is needed.
  const indexUrl = form.dataset.index || "/assets/search-index.json";
  let indexPromise = null;
  const loadIndex = () => {
    indexPromise ??= fetch(indexUrl, { headers: { accept: "application/json" } })
      .then((response) => {
        if (!response.ok) throw new Error(String(response.status));
        return response.json();
      })
      .then((index) => index.documents || []);
    return indexPromise;
  };

  let lastQuery = null;
  const search = async (query) => {
    if (query === lastQuery) return;
    lastQuery = query;
    const url = new URL(location.href);
    if (query.trim()) url.searchParams.set("q", query);
    else url.searchParams.delete("q");
    history.replaceState(null, "", url);
    if (!query.trim()) {
      render([], "");
      return;
    }
    status.textContent = status.dataset.loading || "Searching…";
    try {
      const documents = await loadIndex();
      if (query === lastQuery) render(documents, query);
    } catch {
      status.textContent = status.dataset.failed || "Search is temporarily unavailable.";
    }
  };

  let timer;
  input.addEventListener("input", () => {
    clearTimeout(timer);
    timer = setTimeout(() => void search(input.value), 150);
  });
  input.addEventListener("focus", () => void loadIndex().catch(() => {}));
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    clearTimeout(timer);
    void search(input.value);
  });

  const initial = new URL(location.href).searchParams.get("q") || "";
  input.value = initial;
  if (initial.trim()) void search(initial);
})();
