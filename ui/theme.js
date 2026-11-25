// Light or dark: follows the system until the player picks one with the button in the
// top bar; the choice is kept in localStorage. Loaded in <head> so the page never flashes.
(() => {
  const KEY = "sixlauncher-theme";
  const root = document.documentElement;
  const saved = (() => {
    try { return localStorage.getItem(KEY); } catch { return null; }
  })();
  if (saved === "light" || saved === "dark") root.dataset.theme = saved;

  const current = () => root.dataset.theme || (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");

  document.addEventListener("DOMContentLoaded", () => {
    const button = document.getElementById("theme");
    if (!button) return;
    button.addEventListener("click", () => {
      const next = current() === "dark" ? "light" : "dark";
      root.dataset.theme = next;
      try { localStorage.setItem(KEY, next); } catch { /* private mode: keep it for this run */ }
    });
  });
})();
