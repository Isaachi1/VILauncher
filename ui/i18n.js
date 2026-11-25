// SixLauncher's languages. The window starts in the system's language (navigator.languages,
// which WebKitGTK takes from LANG / LANGUAGE) and the player can pick another one in the top
// bar; the choice is kept in localStorage, like the theme. Each language is one file in
// locales/ that calls I18N.add(code, {key: text}); pt-BR is the source and has every key,
// and a key missing from a language falls back to English, then to Portuguese.
//
// In the page: data-i18n="key" sets the text, data-i18n-attr="placeholder:key;title:key"
// sets attributes. In app.js: t("key", {name: value}) with {name} placeholders, or
// I18N.text(el, "key", vars) for a message that should follow a later change of language
// (I18N.raw(el, text) for text that is not ours: a path, a game's name, an error).
// A placeholder's value may itself be {k: "key", v: vars}, translated when shown.
//
// Loaded in <head>, before the locales and app.js.
(() => {
  const KEY = "sixlauncher-lang";

  // The most spoken languages in the world (native + second-language speakers), each named
  // in itself. dir: "rtl" for the scripts written right to left.
  const LANGS = [
    { code: "en", name: "English" },
    { code: "zh-CN", name: "简体中文" },
    { code: "hi", name: "हिन्दी" },
    { code: "es", name: "Español" },
    { code: "ar", name: "العربية", dir: "rtl" },
    { code: "fr", name: "Français" },
    { code: "bn", name: "বাংলা" },
    { code: "pt-BR", name: "Português (Brasil)" },
    { code: "ru", name: "Русский" },
    { code: "ur", name: "اردو", dir: "rtl" },
    { code: "id", name: "Bahasa Indonesia" },
    { code: "de", name: "Deutsch" },
    { code: "ja", name: "日本語" },
    { code: "tr", name: "Türkçe" },
    { code: "ko", name: "한국어" },
  ];
  const SOURCE = "pt-BR";
  const dicts = {};
  const listeners = [];
  const varsOf = new WeakMap();

  const stored = () => {
    try { return localStorage.getItem(KEY); } catch { return null; }
  };

  // The best of our languages for a list of BCP 47 tags: the exact tag, then the same
  // base language (pt-PT -> pt-BR, zh-TW -> zh-CN, es-MX -> es), else English.
  function pick(prefs) {
    const codes = LANGS.map((l) => l.code);
    for (const raw of prefs) {
      if (!raw) continue;
      const tag = raw.replace("_", "-").split(".")[0];
      const exact = codes.find((c) => c.toLowerCase() === tag.toLowerCase());
      if (exact) return exact;
      const base = tag.split("-")[0].toLowerCase();
      const same = codes.find((c) => c.split("-")[0].toLowerCase() === base);
      if (same) return same;
    }
    return "en";
  }

  const system = () => pick(navigator.languages?.length ? navigator.languages : [navigator.language]);

  const I18N = {
    langs: LANGS,
    lang: SOURCE,
    // "auto" (follow the system) or a code the player picked.
    choice: "auto",

    add(code, dict) {
      dicts[code] = dict;
    },

    t(key, vars) {
      let s = dicts[I18N.lang]?.[key] ?? dicts.en?.[key] ?? dicts[SOURCE]?.[key] ?? key;
      if (vars) {
        s = s.replace(/\{(\w+)\}/g, (m, k) => {
          if (!(k in vars)) return m;
          const v = vars[k];
          return v && typeof v === "object" && "k" in v ? I18N.t(v.k, v.v) : String(v);
        });
      }
      return s;
    },

    // Sets an element's text from a key, and keeps the key so a change of language redraws it.
    text(el, key, vars) {
      el.dataset.i18n = key;
      if (vars) varsOf.set(el, vars);
      else varsOf.delete(el);
      el.textContent = I18N.t(key, vars);
    },

    // Sets text that is not translated, so a change of language leaves it alone.
    raw(el, text) {
      delete el.dataset.i18n;
      varsOf.delete(el);
      el.textContent = text;
    },

    // For Intl: the language with Latin digits, so the scores and bars read the same in
    // every language.
    locale() {
      return `${I18N.lang}-u-nu-latn`;
    },

    dir() {
      return LANGS.find((l) => l.code === I18N.lang)?.dir || "ltr";
    },

    // Translates every marked element under root.
    apply(root = document) {
      const t = I18N.t;
      for (const el of root.querySelectorAll("[data-i18n]")) el.textContent = t(el.dataset.i18n, varsOf.get(el));
      for (const el of root.querySelectorAll("[data-i18n-attr]")) {
        for (const pair of el.dataset.i18nAttr.split(";")) {
          const [attr, key] = pair.split(":").map((s) => s.trim());
          if (attr && key) el.setAttribute(attr, t(key));
        }
      }
    },

    // Switches the language ("auto" follows the system), translates the page and tells
    // app.js so it can redraw what it built.
    set(choice) {
      I18N.choice = choice && choice !== "auto" && LANGS.some((l) => l.code === choice) ? choice : "auto";
      I18N.lang = I18N.choice === "auto" ? system() : I18N.choice;
      try {
        if (I18N.choice === "auto") localStorage.removeItem(KEY);
        else localStorage.setItem(KEY, I18N.choice);
      } catch { /* private mode: keep it for this run */ }
      const root = document.documentElement;
      root.lang = I18N.lang;
      root.dir = I18N.dir();
      if (document.readyState !== "loading") I18N.apply();
      for (const fn of listeners) fn(I18N.lang);
    },

    onChange(fn) {
      listeners.push(fn);
    },

    // Fills the language <select>: "system language" first, then every language by its own name.
    fillPicker(select) {
      select.replaceChildren();
      const auto = document.createElement("option");
      auto.value = "auto";
      auto.textContent = `${I18N.t("lang.auto")} (${LANGS.find((l) => l.code === system())?.name})`;
      select.append(auto);
      for (const l of LANGS) {
        const opt = document.createElement("option");
        opt.value = l.code;
        opt.textContent = l.name;
        opt.lang = l.code;
        if (l.dir) opt.dir = l.dir;
        select.append(opt);
      }
      select.value = I18N.choice;
    },
  };

  window.I18N = I18N;
  window.t = I18N.t;

  // Set the language before the body is drawn (no flash of Portuguese); the locales load
  // after this file, so the page is translated once they are in.
  const saved = stored();
  I18N.choice = saved && LANGS.some((l) => l.code === saved) ? saved : "auto";
  I18N.lang = I18N.choice === "auto" ? system() : I18N.choice;
  document.documentElement.lang = I18N.lang;
  document.documentElement.dir = I18N.dir();
  document.addEventListener("DOMContentLoaded", () => I18N.apply());
})();
