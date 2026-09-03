// Article helpers that need a script: a copy button on every code block.
// Plain script served from /assets/article.js to satisfy the script-src
// 'self' CSP. There is no animation here, so nothing to reduce for readers
// who prefer reduced motion.
(() => {
  "use strict";
  const prose = document.querySelector(".prose[data-label-copy]");
  if (!prose || !navigator.clipboard) return;
  const labelCopy = prose.dataset.labelCopy || "Copy";
  const labelCopied = prose.dataset.labelCopied || "Copied";
  for (const pre of prose.querySelectorAll("pre")) {
    // Captured before the button joins the block, so the label never
    // ends up in the clipboard.
    const text = pre.textContent || "";
    const button = document.createElement("button");
    button.type = "button";
    button.className = "copy-code";
    button.textContent = labelCopy;
    button.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(text);
        button.textContent = labelCopied;
        setTimeout(() => {
          button.textContent = labelCopy;
        }, 1500);
      } catch {
        /* permission denied: the label simply stays */
      }
    });
    pre.append(button);
  }
})();
