// Like button for public articles. Counts are owner-facing only, so the page
// never fetches them: the button reveals itself once this script runs, shows
// only your own remembered state, and POSTs the toggle. Plain script served
// from /assets/like.js to satisfy the script-src 'self' CSP.
(() => {
  "use strict";
  const button = document.querySelector("button[data-content-id]");
  if (!button) return;
  const id = button.dataset.contentId;
  const storageKey = `like:${id}`;

  const liked = () => {
    try {
      return localStorage.getItem(storageKey) === "1";
    } catch {
      return false;
    }
  };
  const remember = (value) => {
    try {
      if (value) localStorage.setItem(storageKey, "1");
      else localStorage.removeItem(storageKey);
    } catch {
      /* private mode: the toggle still works, it just forgets on reload */
    }
  };
  const render = () => {
    button.textContent = liked()
      ? `♥ ${button.dataset.labelLiked || "Liked"}`
      : `♡ ${button.dataset.labelLike || "Like"}`;
  };

  let busy = false;
  button.addEventListener("click", async () => {
    if (busy) return;
    busy = true;
    const wasLiked = liked();
    remember(!wasLiked);
    render();
    try {
      const response = await fetch(`/likes/${id}`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ op: wasLiked ? "unlike" : "like" }),
      });
      if (!response.ok) throw new Error(String(response.status));
    } catch {
      remember(wasLiked);
      render();
    } finally {
      busy = false;
    }
  });

  render();
  button.hidden = false;
})();
