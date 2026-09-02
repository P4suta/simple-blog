// Reader display preferences. Taste differs per person — measure, text size,
// color scheme — so these belong to the visitor, not the site owner: stored
// in localStorage, applied as data attributes the theme CSS keys off.
//
// Loaded synchronously from <head> (tiny, immutable-cached) so a saved
// preference applies before first paint instead of flashing the defaults.
// Without JavaScript the control stays hidden and the defaults simply hold.
(() => {
  "use strict";
  const root = document.documentElement;
  const keys = ["measure", "text", "scheme"];

  const read = (key) => {
    try {
      return localStorage.getItem(`pref:${key}`) || "";
    } catch {
      return "";
    }
  };
  const write = (key, value) => {
    try {
      if (value === "default") localStorage.removeItem(`pref:${key}`);
      else localStorage.setItem(`pref:${key}`, value);
    } catch {
      /* private mode: applies for this page load only */
    }
  };
  const apply = () => {
    for (const key of keys) {
      const value = read(key);
      if (value) root.dataset[key] = value;
      else delete root.dataset[key];
    }
  };

  apply();

  addEventListener("DOMContentLoaded", () => {
    const box = document.querySelector("details.prefs");
    if (!box) return;
    box.hidden = false;
    for (const input of box.querySelectorAll("input[type=radio]")) {
      input.checked = (read(input.name) || "default") === input.value;
      input.addEventListener("change", () => {
        write(input.name, input.value);
        apply();
      });
    }
  });
})();
