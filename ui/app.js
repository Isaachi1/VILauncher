// SixLauncher's window. Everything happens in Rust (sixlauncher-core); this file
// only edits the configuration and shows what comes back.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const dialog = window.__TAURI__.dialog;

const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => [...document.querySelectorAll(sel)];

// Text in the player's language (i18n.js, locales/). say() keeps the key, so the message is
// redrawn when the language changes; plain() is for text that is not ours.
const say = (sel, key, vars) => I18N.text($(sel), key, vars);
const plain = (sel, text) => I18N.raw($(sel), text);

let config = null;
let pendingForce = false;
let statusLoaded = false;
const MAX_LOG_LINES = 5000;

// -- the form <-> the configuration --------------------------------------------------------

function getPath(obj, path) {
  return path.split(".").reduce((o, k) => (o == null ? undefined : o[k]), obj);
}

function setPath(obj, path, value) {
  const keys = path.split(".");
  const last = keys.pop();
  const target = keys.reduce((o, k) => (o[k] ??= {}), obj);
  if (value === null || value === undefined) delete target[last];
  else target[last] = value;
}

function fillForm() {
  for (const el of $$("[data-path]")) {
    const v = getPath(config, el.dataset.path);
    if (el.dataset.kind === "lines") el.value = (v || []).join("\n");
    else if (el.dataset.kind === "env") el.value = Object.entries(v || {}).map(([k, x]) => `${k}=${x}`).join("\n");
    else if (el.dataset.kind === "bool") el.checked = Boolean(v);
    else {
      // A value this window has no option for yet (a GPU not plugged in now) is kept.
      if (el.tagName === "SELECT" && v && ![...el.options].some((o) => o.value === v)) el.appendChild(new Option(v, v));
      el.value = v ?? "";
    }
  }
  refreshVisibility();
}

function readForm() {
  const next = structuredClone(config);
  for (const el of $$("[data-path]")) {
    const raw = el.value;
    let v;
    if (el.dataset.kind === "lines") {
      v = raw.split("\n").map((s) => s.trim()).filter(Boolean);
    } else if (el.dataset.kind === "env") {
      v = {};
      for (const line of raw.split("\n")) {
        const i = line.indexOf("=");
        if (i > 0) v[line.slice(0, i).trim()] = line.slice(i + 1);
      }
    } else if (el.dataset.kind === "bool") {
      v = el.checked;
    } else if (el.dataset.kind === "int") {
      const n = parseInt(raw, 10);
      v = Number.isFinite(n) && n > 0 ? n : null;
    } else if (el.tagName === "SELECT") {
      v = raw;
    } else {
      v = raw.trim() === "" ? null : raw.trim();
    }
    setPath(next, el.dataset.path, v);
  }
  return next;
}

function refreshVisibility() {
  const backend = $('[data-path="backend.kind"]').value;
  const runner = $('[data-path="runner.kind"]').value;
  for (const el of $$(".only-victory")) el.hidden = backend !== "victory";
  for (const el of $$(".only-xcl")) el.hidden = backend !== "xcl";
  for (const el of $$(".only-wine")) el.hidden = runner !== "wine";
  for (const el of $$(".only-mangohud")) el.hidden = !$('[data-path="performance.mangohud"]').checked;
  for (const el of $$(".only-proton")) el.hidden = runner !== "proton";
  for (const el of $$(".only-native")) el.hidden = runner !== "native";
  for (const el of $$(".not-native")) el.hidden = runner === "native";
  // GPU offload, MangoHud, GameMode and Proton's detailed logs are Linux's.
  for (const el of $$(".not-windows")) el.hidden = PLATFORM === "windows";
  say("#runner-path-label", runner === "wine" ? "run.winePath" : "run.protonPath");
  const layer = runner === "native" ? t("run.nativeLabel") : runner === "wine" ? "Wine + vkd3d-proton" : "Proton";
  $("#backend-label").textContent = `${backend === "xcl" ? "XCL" : "VIctory"} · ${layer}`;
}

// The host the window runs on (`linux`, `macos`, `windows`), from get_state.
let PLATFORM = "linux";

// Windows runs the backends natively: only the native runner is offered there, and it is
// offered nowhere else.
function offerRunners(platform) {
  PLATFORM = platform;
  const windows = platform === "windows";
  for (const opt of $$('[data-path="runner.kind"] option')) {
    if ((opt.value === "native") !== windows) opt.remove();
  }
  if (windows) $("#runner-title").textContent = "Windows";
}

async function refreshTitle() {
  const dir = $('[data-path="game.dir"]').value.trim();
  if (!dir) {
    say("#game-title", "hero.noGame");
    say("#game-sub", "hero.noGameSub");
    return;
  }
  try {
    const info = await invoke("game_info", { dir });
    plain("#game-title", info.display_name || info.identity || dir.split("/").pop());
    plain("#game-sub", [info.identity, info.version].filter(Boolean).join(" · ") || dir);
    verifyGame();
  } catch (e) {
    plain("#game-title", dir.split("/").pop() || dir);
    plain("#game-sub", String(e));
  }
}

// -- the game's files against the known SHA-256 (integrity.rs) -----------------------------

const short12 = (h) => (h ? h.slice(0, 12) : "");
let verifyRun = 0;

// The executable, when the folder or the executable changes and the check is on. Hashing a
// large file takes seconds the first time; then the result comes from the launcher's cache.
async function verifyGame() {
  const box = $("#game-verify");
  const form = readForm();
  const run = ++verifyRun;
  box.classList.remove("ok", "missing");
  if (!form.game.verify || !form.game.dir) return plain("#game-verify", "");
  say("#game-verify", "game.verifying");
  try {
    const v = await invoke("verify_game", { config: form });
    if (run !== verifyRun) return; // a newer check started meanwhile
    if (!v) return plain("#game-verify", "");
    const ok = v.known && v.known.kind === "exe";
    box.classList.add(ok ? "ok" : "missing");
    if (ok) say("#game-verify", "game.verifyOk", { name: v.known.name, sha: short12(v.sha256) });
    else say("#game-verify", "game.verifyUnknown", { sha: short12(v.sha256) });
  } catch (e) {
    if (run !== verifyRun) return;
    box.classList.add("missing");
    say("#game-verify", "game.verifyError", { error: String(e) });
  }
}

async function verifyZip() {
  const picked = await dialog.open({ multiple: false, filters: [{ name: "ZIP", extensions: ["zip"] }] });
  if (typeof picked !== "string") return;
  const file = picked.split("/").pop();
  say("#zip-verify", "game.verifying");
  const v = await invoke("verify_file", { path: picked });
  if (v.known && v.known.kind === "zip") say("#zip-verify", "game.zipOk", { file, name: v.known.name, sha: short12(v.sha256) });
  else say("#zip-verify", "game.zipUnknown", { file, sha: short12(v.sha256) });
}

// -- feedback ----------------------------------------------------------------------------

function showBanner(text, canForce) {
  $("#banner-text").textContent = text;
  $("#banner-force").hidden = !canForce;
  pendingForce = canForce;
  $("#banner").hidden = false;
}

function hideBanner() {
  $("#banner").hidden = true;
  pendingForce = false;
}

function selectTab(name) {
  for (const b of $$(".tabs button")) b.setAttribute("aria-selected", String(b.dataset.tab === name));
  for (const p of $$(".panel")) p.hidden = p.id !== `tab-${name}`;
  if (name === "status" && !statusLoaded) {
    statusLoaded = true;
    $("#status-refresh").click();
  }
}

function appendLog(line, stream) {
  const log = $("#log");
  const span = document.createElement("span");
  if (stream === "stderr") span.className = "stderr";
  span.textContent = line + "\n";
  const atBottom = log.scrollTop + log.clientHeight >= log.scrollHeight - 4;
  log.appendChild(span);
  while (log.childNodes.length > MAX_LOG_LINES) log.removeChild(log.firstChild);
  if (atBottom) log.scrollTop = log.scrollHeight;
}

function setRunning(running) {
  $("#play").hidden = running;
  $("#stop").hidden = !running;
  $("#live").classList.toggle("on", running);
  say("#live-text", running ? "live.running" : "live.ready");
}

const STATUS_KEY = { ok: "check.ok", warn: "check.warn", fail: "check.fail" };
let lastChecks = null;

function showChecks(checks) {
  lastChecks = checks;
  const body = $("#checks tbody");
  body.replaceChildren();
  for (const c of checks) {
    const tr = document.createElement("tr");
    const pill = document.createElement("span");
    pill.className = `pill ${c.status}`;
    pill.textContent = t(STATUS_KEY[c.status]);
    const tdS = document.createElement("td");
    tdS.appendChild(pill);
    const tdN = document.createElement("td");
    tdN.className = "name";
    tdN.textContent = c.name;
    const tdD = document.createElement("td");
    tdD.textContent = c.detail;
    if (c.fix) {
      const fix = el("button", "fix primary");
      I18N.text(fix, "fix.button");
      fix.addEventListener("click", () => askFix(c.fix));
      tdD.appendChild(fix);
    }
    tr.append(tdS, tdN, tdD);
    body.appendChild(tr);
  }
  const fails = checks.filter((c) => c.status === "fail").length;
  const warns = checks.filter((c) => c.status === "warn").length;
  say("#doctor-status", "doctor.summary", { fails, warns, ok: checks.length - fails - warns });
}

// A kernel setting's fix: say what it writes (and the risk, for userfaultfd), then run it as
// root (pkexec asks for the password) and check again.
let pendingFix = null;

function askFix(plan) {
  pendingFix = plan;
  say("#fix-title", "fix.title", { setting: plan.setting });
  say("#fix-what", "fix.what", { setting: plan.setting, value: plan.value, file: plan.file });
  $("#fix-risk").hidden = plan.id !== "userfaultfd";
  plain("#fix-manual", plan.manual);
  $("#fix-dialog").showModal();
}

async function applyFix(plan) {
  say("#doctor-status", "fix.busy", { setting: plan.setting });
  await invoke("apply_fixes", { fixes: [plan.id] });
  showChecks(await invoke("run_doctor", { config: readForm() }));
  say("#doctor-status", "fix.done", { setting: plan.setting, value: plan.value });
}

// -- the report for the backends' developers ------------------------------------------------

// What "detailed logs" adds to the run's environment: Proton writes steam-*.log (with
// vkd3d-proton's messages) when PROTON_LOG is set; with Wine, vkd3d-proton and Wine log to the
// run's output.
const VERBOSE_ENV = {
  proton: { PROTON_LOG: "1" },
  wine: { VKD3D_DEBUG: "warn", WINEDEBUG: "+seh,+loaddll,err+all" },
};

function verboseOn(cfg) {
  const env = cfg.env || {};
  return Object.values(VERBOSE_ENV).some((vars) => Object.entries(vars).every(([k, v]) => env[k] === v));
}

async function setVerbose(on) {
  const next = readForm();
  next.env ??= {};
  for (const vars of Object.values(VERBOSE_ENV)) for (const [k, v] of Object.entries(vars)) if (next.env[k] === v) delete next.env[k];
  if (on) Object.assign(next.env, VERBOSE_ENV[next.runner.kind === "wine" ? "wine" : "proton"]);
  config = next;
  fillForm();
  await save();
  say("#report-status", on ? "report.verboseOn" : "report.verboseOff");
}

async function exportReport() {
  const defaultPath = await invoke("report_default_path");
  const path = await dialog.save({ defaultPath, filters: [{ name: "ZIP", extensions: ["zip"] }] });
  if (typeof path !== "string") return;
  say("#report-status", "report.busy");
  const w = await invoke("export_report", { config: readForm(), path });
  say("#report-status", "report.done", { path: w.path, n: w.files.length, mb: (w.bytes / 1048576).toFixed(1) });
}

// -- status (PortWatch VI) --------------------------------------------------------------------

const BACKEND_NAME = { victory: "VIctory", xcl: "XCL" };

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

const SVG = "http://www.w3.org/2000/svg";

function svgEl(tag, attrs = {}) {
  const e = document.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  return e;
}

const pct = (v) => (Math.round(v * 10) / 10).toLocaleString(I18N.locale());
let milestones = [];
// The steps come from the core with Portuguese labels; ours are by key.
const msLabel = (key) => {
  const own = t(`ms.${key}`);
  return own !== `ms.${key}` ? own : (milestones.find((m) => m[0] === key) || [key, key])[1];
};

function lockIcon() {
  const s = svgEl("svg", { viewBox: "0 0 16 16", class: "lock", "aria-hidden": "true" });
  s.append(svgEl("path", { d: "M4.5 7V5a3.5 3.5 0 0 1 7 0v2h.5a1 1 0 0 1 1 1v5.5a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V8a1 1 0 0 1 1-1h.5Zm1.6 0h3.8V5a1.9 1.9 0 0 0-3.8 0v2Z" }));
  return s;
}

function countUp(node, to) {
  const start = performance.now();
  const step = (now) => {
    const t = Math.min(1, (now - start) / 900);
    const v = to * (1 - Math.pow(1 - t, 3));
    node.textContent = t < 1 ? String(Math.round(v)) : pct(to);
    if (t < 1) requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
}

// The headline card, after PortWatch VI's own: the leader's score, four numbers, the
// whole scale as one bar with the best efforts above it, and the leader's score by day.
function showOverall(report) {
  const o = report.overall;
  milestones = o.milestones;
  const byId = new Map(report.efforts.map((e) => [e.id, e]));
  const leader = o.leader ? byId.get(o.leader) : null;

  countUp($("#ov-score"), o.score);
  const line = $("#ov-leader");
  line.replaceChildren();
  if (leader) {
    line.append(`${t("ov.leader")} `, el("b", null, leader.name), " ");
    if (leader.repo) line.append(`${t("ov.leaderOf", { owner: leader.repo.split("/")[0] })} `);
    else if (leader.based_on.length) {
      const priv = el("span", "ov-private");
      priv.append(lockIcon(), `${t("ov.privateBased")} `, el("b", null, leader.based_on.join(", ")));
      line.append(priv, t("ov.sep"));
    }
    line.append(leader.reached === "groundwork" ? t("ov.groundwork") : t("ov.reached", { stage: msLabel(leader.reached) }));
  } else line.textContent = t("ov.none");

  $("#ov-stage").textContent = msLabel(o.furthest);
  $("#ov-top3").textContent = `${pct(o.top3)}%`;
  $("#ov-boot").textContent = `${o.booted} / ${o.tracked}`;
  $("#ov-count").textContent = String(o.tracked);

  const furthest = milestones.findIndex((m) => m[0] === o.furthest);
  const ticks = $("#mega-ticks");
  ticks.replaceChildren();
  for (const m of milestones.slice(1, -1)) {
    const i = el("i");
    i.style.left = `${m[2]}%`;
    ticks.append(i);
  }
  const labels = $("#mega-labels");
  labels.replaceChildren();
  milestones.forEach((m, i) => {
    const cls = i <= furthest ? "is-reached" : i === furthest + 1 ? "is-next" : "";
    const label = el("div", `mega-label ${cls}`);
    label.style.left = `${m[2]}%`;
    label.append(el("b", null, `${m[2]}%`), el("span", "dot"), msLabel(m[0]));
    labels.append(label);
  });
  labels.append(el("p", "mega-zone", t("ov.tracks")));

  const fill = $("#mega-fill");
  fill.style.setProperty("--p", "0%");
  requestAnimationFrame(() => requestAnimationFrame(() => fill.style.setProperty("--p", `${o.score}%`)));

  const draw = () => {
    drawMarkers(o.top, o.leader);
    layoutLabels();
  };
  draw();
  document.fonts?.ready.then(draw);
  showHistory(o.history);
}

// Markers sit in one row above the bar, spread so they never overlap, with a curved
// line down to each effort's true place. Initials stand in for avatars: the launcher
// loads nothing from other sites.
function drawMarkers(top, leaderId) {
  const box = $("#mega-markers");
  const W = box.clientWidth;
  const H = box.clientHeight;
  if (!W) return;
  const GAP = 42;
  const items = top.map((m) => ({ m, x: (m.score / 100) * W })).sort((a, b) => a.x - b.x);
  items.forEach((it, i) => { it.a = i ? Math.max(it.x, items[i - 1].a + GAP) : it.x; });
  const shift = items.reduce((s, it) => s + (it.x - it.a), 0) / Math.max(1, items.length);
  items.forEach((it, i) => {
    it.a += shift;
    it.a = Math.max(18, i ? Math.max(it.a, items[i - 1].a + GAP) : it.a);
  });
  const overflow = items.length ? items[items.length - 1].a - (W - 18) : 0;
  if (overflow > 0) items.forEach((it) => { it.a -= overflow; });

  box.replaceChildren();
  const svg = svgEl("svg", { width: W, height: H, class: "mk-lines", "aria-hidden": "true" });
  const y0 = H - 34;
  for (const it of items) {
    const lead = it.m.id === leaderId;
    const kind = lead ? "lead" : it.m.private ? "priv" : "pub";
    const path = svgEl("path", { d: `M${it.a} ${y0} C ${it.a} ${y0 + 16}, ${it.x} ${H - 16}, ${it.x} ${H}`, class: `mk-line ${kind}` });
    svg.append(path, svgEl("circle", { cx: it.x, cy: H - 1, r: 3, class: `mk-dot ${kind}` }));
  }
  box.append(svg);
  items.forEach((it, i) => {
    const lead = it.m.id === leaderId;
    const mk = el("span", `marker${lead ? " is-leader" : ""}${it.m.private ? " is-private" : ""}`);
    mk.style.left = `${it.a}px`;
    mk.style.setProperty("--i", i);
    mk.title = `${it.m.name} · ${pct(it.m.score)}%`;
    mk.append(el("span", "mk-name", `${it.m.name} · ${pct(it.m.score)}%`));
    const face = el("span", "mk-face");
    face.append(avatarFace(it.m.id, it.m.name, "mk-initials"));
    if (it.m.private) {
      const badge = el("span", "mk-lock");
      badge.append(lockIcon());
      face.append(badge);
    }
    mk.append(face);
    box.append(mk);
  });
}

// The step names sit under their ticks. When neighbours would touch, they are dealt into
// the fewest rows that keep each row clear (1-2-1-2, then 1-2-3…), so each row reads left to right.
function layoutLabels() {
  const box = $("#mega-labels");
  const labels = [...box.querySelectorAll(".mega-label")];
  if (!labels.length || !box.clientWidth) return;
  for (const l of labels) l.style.top = "0px";
  const span = (e) => {
    const r = e.getBoundingClientRect();
    return [r.left, r.right];
  };
  const spans = labels.map(span);
  const zone = box.querySelector(".mega-zone");
  const zoneSpan = zone && zone.getClientRects().length ? span(zone) : null;
  const last = labels.length - 1;
  const clear = (a, b) => a[1] + 10 <= b[0] || b[1] + 10 <= a[0];
  const fits = (rows) =>
    spans.every((a, i) => (i % rows || i === last || !zoneSpan || clear(a, zoneSpan)) && spans.every((b, j) => j <= i || (j - i) % rows || clear(a, b)));
  let rows = 1;
  while (rows < labels.length && !fits(rows)) rows++;
  const h = Math.max(...labels.map((l) => l.offsetHeight));
  labels.forEach((l, i) => {
    if (i !== last) l.style.top = `${(i % rows) * (h + 4)}px`;
  });
  box.style.height = `${rows * (h + 4)}px`;
}

function showHistory(history) {
  const box = $("#ov-history");
  box.hidden = history.length < 2;
  if (box.hidden) return;
  const W = 1000, H = 70;
  const max = Math.max(30, ...history.map((p) => p.score)) * 1.1;
  const x = (i) => (i / (history.length - 1)) * W;
  const y = (v) => H - (v / max) * (H - 6) - 3;
  const d = history.map((p, i) => `${i ? "L" : "M"}${x(i).toFixed(1)} ${y(p.score).toFixed(1)}`).join(" ");
  $("#ov-line").setAttribute("d", d);
  $("#ov-area").setAttribute("d", `${d} L${W} ${H} L0 ${H} Z`);
}

window.addEventListener("resize", () => {
  if (lastReport && !$("#tab-status").hidden) {
    drawMarkers(lastReport.overall.top, lastReport.overall.leader);
    layoutLabels();
  }
});

let lastReport = null;

// Pictures by effort id, as data: URLs from the Rust side (it downloads and caches them).
const avatars = new Map();

function initials(name) {
  return name.replace(/[^\p{L}\p{N}]/gu, "").slice(0, 2).toUpperCase() || "?";
}

// The effort's picture when we have it, its initials until then (or for good).
function avatarFace(id, name, cls) {
  const face = el("span", cls);
  face.dataset.avatar = id;
  face.dataset.initials = initials(name);
  paintFace(face);
  return face;
}

function paintFace(face) {
  const src = avatars.get(face.dataset.avatar);
  if (src) {
    const img = el("img");
    img.alt = "";
    img.src = src;
    face.replaceChildren(img);
    face.classList.add("has-img");
  } else face.textContent = face.dataset.initials;
}

async function loadAvatars(report) {
  const wanted = report.efforts.filter((e) => e.avatar_url && !avatars.has(e.id)).map((e) => ({ id: e.id, url: e.avatar_url }));
  if (!wanted.length) return;
  const got = await invoke("portwatch_avatars", { wanted });
  for (const [id, src] of Object.entries(got)) avatars.set(id, src);
  for (const face of $$("[data-avatar]")) paintFace(face);
}

// The core names an effort's furthest step in Portuguese (status.rs, Milestones::stage).
const STAGE_KEY = { Story: "story", "Jogável": "playable", "No mundo": "world", Menus: "menu", Inicia: "boots", "Carrega o executável": "loads" };
const stageLabel = (stage) => (STAGE_KEY[stage] ? msLabel(STAGE_KEY[stage]) : stage);

function showEfforts(report) {
  lastReport = report;
  showOverall(report);
  loadAvatars(report).catch(() => {});
  const box = $("#efforts");
  box.replaceChildren();
  const when = report.updated ? new Date(report.updated).toLocaleString(I18N.locale()) : "?";
  if (report.stale) say("#status-info", "status.stale", { when, reason: report.stale });
  else say("#status-info", "status.updated", { when });
  let rank = 0;
  for (const e of report.efforts) {
    const watch = e.tier === "watchlist";
    const card = el("article", `effort${e.backend ? " ours" : ""}${watch ? " watch" : ""}`);
    card.append(el("span", "rank", watch ? "–" : String(++rank)));
    const body = el("div", "effort-body");
    const head = el("header");
    const title = el("div", "effort-title");
    const who = el("div", "who");
    const names = el("div");
    const h3 = el("h3", null, e.name);
    if (e.private) h3.append(lockIcon());
    names.append(h3);
    names.append(el("p", "repo", e.repo ? `github.com/${e.repo}` : e.based_on.length ? t("effort.privateBased", { names: e.based_on.join(", ") }) : t("effort.closed")));
    who.append(avatarFace(e.id, e.name, "e-avatar"), names);
    title.append(who);
    const chips = el("div", "chips");
    if (e.backend) chips.append(el("span", "pill violet", t("effort.backend", { name: BACKEND_NAME[e.backend] })));
    if (e.partial) chips.append(el("span", "pill warn", t("effort.partial", { stage: stageLabel(e.stage) })));
    if (e.gta.fps > 0) chips.append(el("span", "pill gold", `~${e.gta.fps} fps`));
    if (watch) chips.append(el("span", "pill", t("effort.watch")));
    if (chips.childElementCount) title.append(chips);
    const score = el("div", "e-score");
    const big = el("b", null, pct(e.score));
    big.append(el("small", null, "%"));
    score.append(big, el("span", null, msLabel(e.reached)));
    head.append(title, score);
    body.append(head, effortBar(e));
    const lists = el("div", "lists");
    for (const [list, cls] of [[e.highlights, "good"], [e.blockers, "bad"]]) {
      if (!list.length) continue;
      const ul = el("ul", cls);
      for (const item of list) ul.append(el("li", null, item));
      lists.append(ul);
    }
    if (lists.childElementCount) body.append(lists);
    card.append(body);
    box.append(card);
  }
}

// Each effort on the same scale as the headline bar: one gradient revealed up to its
// score, ticks at every step, and the steps it reached named below.
function effortBar(e) {
  const wrap = el("div", "e-bar");
  const track = el("div", "bar-track");
  const fill = el("div", "bar-fill");
  fill.style.setProperty("--p", `${e.score}%`);
  const ticks = el("div", "bar-ticks");
  for (const m of milestones.slice(1, -1)) {
    const i = el("i");
    i.style.left = `${m[2]}%`;
    ticks.append(i);
  }
  track.append(fill, ticks);
  const reached = milestones.findIndex((m) => m[0] === e.reached);
  const legend = el("div", "legend");
  milestones.slice(0, Math.max(1, reached + 1)).forEach((m) => legend.append(el("span", "hit", msLabel(m[0]))));
  wrap.append(track, legend);
  return wrap;
}

// -- runtime updates ---------------------------------------------------------------------------

const short = (c) => (c ? c.slice(0, 10) : "—");

const SCAN_KEYS = ["clean", "flagged", "pending", "skipped", "error"];
const SCAN_PILL = { clean: "ok", flagged: "fail" };

function sourceLine(a) {
  if (!a) return "";
  return [a.effort, a.repo ? `github.com/${a.repo}` : null, short(a.commit)].filter(Boolean).join(" · ");
}

let lastRuntimes = null;

// The Backend list: every build SixLauncher Builder published (runtimes.json), best first, as
// "project · family · score". Picking one installs it if needed and makes it the backend;
// "your own executable" leaves the path below as it is. Under it, #build-detail shows the
// picked build: VirusTotal, size, an update of the same project, the builds it replaced.
const CUSTOM_BUILD = "custom";

const pctOf = (score) =>
  score != null ? `${score.toLocaleString(document.documentElement.lang || undefined, { maximumFractionDigits: 1 })}%` : "";

// The build of a backend in use: same project as the installed one (an older commit of it is
// still that build, with an update), or the same commit for a manifest without repositories.
function inUseChoice(r) {
  const repo = r.source && r.source.repo;
  return (r.choices || []).find((c) => (repo && c.repo === repo) || c.commit === r.installed);
}

function showBuilds(rt) {
  const sel = $("#backend-build");
  const kind = $('[data-path="backend.kind"]').value;
  const exe = $('[data-path="backend.exe"]').value.trim();
  sel.replaceChildren();
  let selected = CUSTOM_BUILD;
  const all = [];
  for (const r of rt ? rt.backends : []) for (const c of r.choices || []) all.push({ r, c });
  all.sort((a, b) => (b.c.score ?? -1) - (a.c.score ?? -1));
  for (const { r, c } of all) {
    const mine = r.exe && inUseChoice(r) === c;
    const tag = mine ? (r.installed === c.commit ? t("runtimes.installedPill") : t("compat.updatePill")) : "";
    const parts = [c.effort || c.repo || short(c.commit), BACKEND_NAME[r.backend], pctOf(c.score), tag].filter(Boolean);
    const opt = el("option", null, parts.join(" · "));
    opt.value = `${r.backend}|${c.file}`;
    opt.title = sourceLine(c);
    sel.append(opt);
    if (mine && r.backend === kind && r.exe === exe) selected = opt.value;
  }
  if (!all.length) {
    const none = el("option", null, t(rt ? "compat.noBuilds" : "compat.loadingBuilds"));
    none.value = "";
    none.disabled = true;
    sel.append(none);
  }
  const own = el("option", null, t("compat.customBuild", { name: BACKEND_NAME[kind] }));
  own.value = CUSTOM_BUILD;
  sel.append(own);
  sel.value = selected;
  showBuildDetail(rt);
}

function picked(rt) {
  const [backend, file] = $("#backend-build").value.split("|");
  const r = rt && file ? rt.backends.find((b) => b.backend === backend) : null;
  const c = r && (r.choices || []).find((x) => x.file === file);
  return r && c ? { r, c } : null;
}

async function pickBuild(sel) {
  const p = picked(lastRuntimes);
  if (!p) return showBuildDetail(lastRuntimes);
  const { r, c } = p;
  // Already installed (this commit): just use it. Otherwise download it.
  if (r.exe && r.installed === c.commit) await useRuntime(r.backend, r.exe);
  else await installRuntime(r.backend, null, false, c.file);
}

function scanLine(scan) {
  const line = el("div", null);
  const pill = el("span", `pill ${SCAN_PILL[scan.verdict] || "warn"}`, SCAN_KEYS.includes(scan.verdict) ? t(`scan.${scan.verdict}`) : scan.verdict);
  const flagged = scan.files.filter((f) => f.malicious > 0).map((f) => `${f.name}: ${f.malicious}/${f.engines}`);
  line.append(pill, " ", flagged.length ? flagged.join(", ") : scan.files.map((f) => `${f.name} ${f.malicious}/${f.engines}`).join(" · "));
  return line;
}

function showBuildDetail(rt) {
  const box = $("#build-detail");
  box.replaceChildren();
  const p = picked(rt);
  const kind = $('[data-path="backend.kind"]').value;
  const r = p ? p.r : rt && rt.backends.find((b) => b.backend === kind);
  if (!p) {
    // Your own executable: nothing to show but what replaced builds of this family there are.
    if (r && r.previous && r.previous.length) box.append(previousBuilds(r));
    box.hidden = !box.childElementCount;
    return;
  }
  const { c } = p;
  box.hidden = false;
  const head = el("div", "build-head");
  const what = el("div");
  what.append(el("div", "build-name", `${c.effort || c.repo} · ${BACKEND_NAME[r.backend]}${c.score != null ? ` · ${pctOf(c.score)}` : ""}`));
  const where = [c.repo ? `github.com/${c.repo}` : null, short(c.commit), `${(c.size / 1048576).toFixed(1)} MB`];
  what.append(el("div", "status", where.filter(Boolean).join(" · ")));
  head.append(what);
  const inUse = r.exe && r.installed === c.commit && $('[data-path="backend.exe"]').value.trim() === r.exe;
  const sameProject = r.exe && inUseChoice(r) === c;
  if (sameProject && r.installed !== c.commit) {
    const b = el("button", "primary", t("runtimes.update"));
    b.addEventListener("click", () => guarded(b, () => installRuntime(r.backend, b, false, c.file)));
    head.append(b);
  } else if (inUse) {
    head.append(el("span", "pill ok", t("runtimes.inUse")));
  }
  box.append(head);
  if (sameProject && r.installed !== c.commit) {
    box.append(el("div", "status", t("runtimes.installed", { build: sourceLine(r.source) || short(r.installed) })));
  }
  if (c.virustotal) box.append(scanLine(c.virustotal));
  if (r.previous && r.previous.length) box.append(previousBuilds(r));
}

function showRuntimes(rt) {
  lastRuntimes = rt;
  showBuilds(rt);
  if (rt.offline) {
    console.warn("runtimes:", rt.offline); // the detail (address, network error) is for debugging
    say("#runtimes-info", "runtimes.offline");
  } else if (rt.unpublished) {
    say("#runtimes-info", "runtimes.unpublished", { url: rt.unpublished });
  }
  const cands = $("#runtimes-candidates");
  cands.replaceChildren();
  for (const c of rt.candidates) cands.append(el("li", null, `${c.name} (${c.repo}): ${c.result}`));
  $("#runtimes-why").hidden = rt.candidates.length === 0;
}

function previousBuilds(r) {
  const box = el("details", "previous");
  box.append(el("summary", null, t("runtimes.previous", { n: r.previous.length })));
  for (const p of r.previous) {
    const row = el("div", "previous-row");
    const what = sourceLine(p.source) || short(p.commit || p.id);
    const text = el("span", null, p.replaced_at ? `${what} · ${t("runtimes.replacedAt", { date: when(p.replaced_at) })}` : what);
    const b = el("button", "ghost", t("runtimes.rollback"));
    b.addEventListener("click", () =>
      guarded(b, async () => {
        const exe = await invoke("runtime_rollback", { backend: r.backend, id: p.id });
        await useRuntime(r.backend, exe);
        say("#runtimes-info", "runtimes.rolledBack", { name: BACKEND_NAME[r.backend], build: what });
      }),
    );
    row.append(text, b);
    box.append(row);
  }
  return box;
}

function useRuntime(backend, exe) {
  $('[data-path="backend.kind"]').value = backend;
  $('[data-path="backend.exe"]').value = exe;
  refreshVisibility();
  return save().then(() => $("#runtimes-check").click());
}

async function installRuntime(backend, button, force = false, file = null) {
  say("#runtimes-info", "runtimes.downloading", { name: BACKEND_NAME[backend] });
  const r = await invoke("install_runtime", { backend, force, file });
  showVerdicts(r.verdicts);
  if (r.blocked) {
    say("#runtimes-info", "runtimes.notInstalled", { name: BACKEND_NAME[backend] });
    showBanner(`${r.blocked}\n\n${t("runtimes.keptOld")}`, false);
    const again = el("button", "danger", t("runtimes.installAnyway"));
    again.addEventListener("click", () => guarded(again, () => installRuntime(backend, again, true, file)));
    if (button) button.replaceWith(again);
    else $("#runtimes-info").after(again);
    if (lastRuntimes) showBuilds(lastRuntimes);
    return;
  }
  const vt = { k: r.checked ? "runtimes.vtChecked" : "runtimes.vtNone" };
  say("#runtimes-info", "runtimes.installedAt", { name: BACKEND_NAME[backend], exe: r.exe, vt });
  if (button) button.hidden = true;
  await useRuntime(backend, r.exe);
}

// -- Proton versions ---------------------------------------------------------------------------

let protonList = [];
let protonReleases = null;

function showProtons(list) {
  protonList = list;
  const datalist = $("#protons");
  datalist.replaceChildren();
  for (const p of list) {
    const opt = document.createElement("option");
    opt.value = p.path;
    opt.label = p.version;
    datalist.appendChild(opt);
  }
  const body = $("#proton-list tbody");
  body.replaceChildren();
  if (!list.length) {
    const tr = el("tr");
    tr.append(el("td", "status", t("proton.none")));
    body.append(tr);
  }
  const chosen = $('[data-path="runner.path"]').value.trim();
  list.forEach((p, i) => {
    const tr = el("tr");
    const name = el("td", "name");
    name.append(el("div", null, p.name), el("div", "status", p.path));
    tr.append(name);
    const origin = el("td");
    origin.append(el("span", `pill ${p.origin === "launcher" ? "violet" : "ok"}`, t(`proton.origin.${p.origin}`)));
    tr.append(origin);
    const td = el("td");
    const inUse = chosen ? chosen === p.path : i === 0;
    if (inUse) td.append(el("span", "pill ok", t(chosen ? "proton.inUse" : "proton.auto")));
    else {
      const b = el("button", null, t("proton.use"));
      b.addEventListener("click", () => guarded(b, () => useProton(p)));
      td.append(b);
    }
    if (p.origin === "launcher" && !(chosen && inUse)) {
      const del = el("button", "ghost", t("proton.remove"));
      del.addEventListener("click", () =>
        guarded(del, async () => {
          await invoke("proton_remove", { name: p.name, config: readForm() });
          say("#proton-info", "proton.removed", { name: p.name });
          await refreshProtons();
        }),
      );
      td.append(" ", del);
    }
    tr.append(td);
    body.append(tr);
  });
}

async function refreshProtons() {
  showProtons(await invoke("list_protons"));
  if (protonReleases) showReleases(protonReleases);
}

async function useProton(p) {
  $('[data-path="runner.kind"]').value = "proton";
  $('[data-path="runner.path"]').value = p.path;
  refreshVisibility();
  await save();
  say("#proton-info", "proton.selected", { name: p.name });
  showProtons(protonList);
}

function showReleases(list) {
  protonReleases = list;
  const have = new Set(protonList.filter((p) => p.origin === "launcher").map((p) => p.name));
  const sel = $("#proton-releases");
  sel.replaceChildren();
  for (const r of list) {
    const date = r.published_at ? new Date(r.published_at).toLocaleDateString(I18N.locale()) : "—";
    const label = t("proton.release", { name: r.name, date, mb: Math.round(r.size / 1048576) });
    const opt = el("option", null, have.has(r.name) ? t("proton.have", { release: label }) : label);
    opt.value = r.name;
    opt.disabled = have.has(r.name);
    sel.append(opt);
  }
  const first = list.find((r) => !have.has(r.name));
  if (first) sel.value = first.name;
  sel.hidden = list.length === 0;
  $("#proton-get").hidden = !first;
}

async function installProton() {
  const name = $("#proton-releases").value;
  if (!name) return;
  say("#proton-info", "proton.downloading", { name });
  const p = await invoke("proton_install", { name });
  await refreshProtons();
  await useProton(p);
  say("#proton-info", "proton.installed", { name: p.name });
}

// -- VirusTotal ----------------------------------------------------------------------------------

const VT_PILL = { clean: "ok", flagged: "fail", unknown: "warn" };
let lastVerdicts = null;
let vtSaved = false;

function showVerdicts(list) {
  lastVerdicts = list;
  const body = $("#vt tbody");
  body.replaceChildren();
  for (const v of list) {
    const tr = el("tr");
    const pill = el("span", `pill ${VT_PILL[v.state]}`, t(`vt.${v.state}`));
    const tdS = el("td");
    tdS.append(pill);
    tr.append(tdS, el("td", "name", v.file));
    let detail;
    if (v.state === "unknown") detail = t("vt.unknownDetail");
    else detail = t("vt.detail", { malicious: v.malicious, engines: v.engines, suspicious: v.suspicious, link: v.link });
    const tdD = el("td", null, detail);
    if (v.state === "unknown") {
      const up = el("button", "ghost", t("vt.upload"));
      up.title = t("vt.uploadTitle");
      up.addEventListener("click", () =>
        guarded(up, async () => {
          await invoke("virustotal_upload", { config: readForm(), file: v.file });
          up.replaceWith(el("span", "status", ` ${t("vt.uploaded")}`));
        }),
      );
      tdD.append(" ", up);
    }
    tr.append(tdD);
    body.append(tr);
  }
}

function showVtKey(saved) {
  vtSaved = saved;
  $("#vt-key").value = "";
  $("#vt-key").dataset.i18nAttr = `placeholder:${saved ? "vt.keySavedPh" : "vt.keyPh"}`;
  $("#vt-key").placeholder = t(saved ? "vt.keySavedPh" : "vt.keyPh");
  $("#vt-check").disabled = !saved;
  say("#vt-info", saved ? "vt.infoSaved" : "vt.infoNone");
}

// -- GPU, MangoHud and GameMode ------------------------------------------------------------

let lastHardware = null;

// The GPUs found go in the "GPU que roda o jogo" list; "auto" says which one it picks.
function showHardware(hw) {
  lastHardware = hw;
  const select = $("#gpu-device");
  const current = select.value || getPath(config, "gpu.device") || "auto";
  for (const o of [...select.options]) if (o.value !== "auto" && o.value !== "system") select.removeChild(o);
  for (const g of hw.gpus) select.appendChild(new Option(`${g.name} · ${g.slot}`, g.slot));
  const auto = hw.gpus.find((g) => g.slot === hw.auto);
  const autoOpt = select.querySelector('option[value="auto"]');
  if (auto) I18N.text(autoOpt, "gpu.autoIs", { name: auto.name });
  else I18N.text(autoOpt, "gpu.auto");
  if (![...select.options].some((o) => o.value === current)) {
    select.appendChild(new Option(t("gpu.missing", { slot: current }), current));
  }
  select.value = current;
  showTool("#mangohud-info", hw.tools.mangohud, "mangohud", hw.tools.sandboxed);
  showTool("#gamemode-info", hw.tools.gamemode, "gamemode", hw.tools.sandboxed);
}

function showTool(sel, tool, pkg, sandboxed) {
  const node = $(sel);
  node.classList.toggle("missing", !tool.installed);
  if (tool.installed) say(sel, sandboxed ? "perf.foundHost" : "perf.found");
  else if (tool.flatpak.length) say(sel, "perf.flatpakOnly", { ext: tool.flatpak.join(", "), pkg });
  else say(sel, "perf.missing", { pkg });
}

async function loadHardware() {
  try {
    showHardware(await invoke("hardware"));
  } catch (e) {
    console.warn("hardware:", e);
  }
}

// -- language ------------------------------------------------------------------------------

// The footer's keyboard prompts, in the player's language.
function showPrompts() {
  const box = $(".prompts");
  if (!box) return;
  const kbd = (k) => el("kbd", null, k);
  box.replaceChildren(
    kbd("Ctrl"), kbd("Enter"), ` ${t("play")} `,
    kbd("Ctrl"), kbd("D"), ` ${t("doctor.run")} `,
    kbd("Ctrl"), kbd("1-5"), ` ${t("keys.tabs")}`,
  );
}

// A new language: the page's marked text is already redrawn (i18n.js); this redraws what
// app.js built from data.
function relabel() {
  I18N.fillPicker($("#lang"));
  showPrompts();
  refreshVisibility();
  refreshTitle();
  if (lastChecks) showChecks(lastChecks);
  if (lastRuntimes) showRuntimes(lastRuntimes);
  showProtons(protonList);
  if (protonReleases) showReleases(protonReleases);
  if (lastVerdicts) showVerdicts(lastVerdicts);
  showVtKey(vtSaved);
  if (lastReport) showEfforts(lastReport);
  if (lastHardware) showHardware(lastHardware);
}

// -- actions -----------------------------------------------------------------------------

async function save() {
  config = readForm();
  const path = await invoke("save_config", { config });
  say("#save-status", "save.done", { path });
  $("#verbose-logs").checked = verboseOn(config);
  return config;
}

async function guarded(button, fn) {
  button.disabled = true;
  try {
    await fn();
  } catch (e) {
    showBanner(String(e), false);
  } finally {
    button.disabled = false;
  }
}

async function play(force) {
  hideBanner();
  await save();
  try {
    $("#log").replaceChildren();
    await invoke("launch_game", { config, force });
    setRunning(true);
    selectTab("log");
  } catch (e) {
    showBanner(`${t("play.cannot")}\n${e}`, !force);
  }
}

// -- SixLauncher's own updates ----------------------------------------------------------------
//
// At start and every 6 hours: if a newer release exists, an AppImage downloads and installs it
// by itself (no password) and offers a restart; the .deb / .rpm install it on a click (pkexec asks
// for the password); on Windows the installer's copy runs the new installer on a click (the window
// closes while it installs); the Flatpak downloads the signed bundle and installs it with
// `flatpak install` on a click; the tarball or the .zip shows where to download it.

const UPDATE_EVERY_MS = 6 * 60 * 60 * 1000;
let updateBusy = false;
let pendingUpdate = null; // the update the "Atualizar" button installs
let shownVersion = null; // the version the notice is about
let dismissedVersion = null; // "Depois": not shown again until a newer one
let installedVersion = null; // installed, runs after a restart

function showUpdate(version, { title, sub = "", url = "", go = false, restart = false }) {
  shownVersion = version;
  $("#update-title").textContent = title;
  $("#update-sub").textContent = sub;
  $("#update-url").textContent = url;
  $("#update-url").hidden = !url;
  $("#update-go").hidden = !go;
  $("#update-restart").hidden = !restart;
  $("#update").hidden = false;
}

function manualHint(u) {
  if (u.install === "flatpak") return "Baixe sixlauncher.flatpak da release e rode: flatpak install --user sixlauncher.flatpak";
  if (u.install === "deb" || u.install === "rpm") return `Baixe o .${u.install} da release e instale por cima do atual.`;
  if (u.install === "appimage") return "Baixe o novo AppImage da release e troque o arquivo atual por ele.";
  if (u.install === "nsis") return "Baixe o instalador (SixLauncher_…_x64-setup.exe) da release e rode-o: ele atualiza a cópia instalada.";
  return "Baixe o novo pacote da release (.deb, .rpm, AppImage, Flatpak, .tar.gz, ou no Windows o instalador .exe).";
}

async function installUpdate(u) {
  if (updateBusy) return;
  updateBusy = true;
  $("#update-go").disabled = true;
  showUpdate(u.latest, { title: `Baixando o SixLauncher ${u.latest}…`, sub: u.install === "appimage" ? "" : u.install === "flatpak" ? "O Flatpak novo é instalado pelo flatpak do sistema; numa instalação do sistema ele pode pedir sua senha." : u.install === "nsis" ? "O instalador abre em seguida e o SixLauncher fecha enquanto ele atualiza." : "O sistema vai pedir sua senha para instalar o pacote." });
  try {
    const version = await invoke("app_update_install");
    if (!version) {
      $("#update").hidden = true;
      return;
    }
    installedVersion = version;
    showUpdate(version, { title: `SixLauncher ${version} instalado`, sub: "A nova versão abre na próxima vez que o SixLauncher iniciar.", restart: true });
  } catch (e) {
    showUpdate(u.latest, { title: `Não foi possível atualizar para o ${u.latest}`, sub: `${e}. ${manualHint(u)}`, url: u.url });
  } finally {
    updateBusy = false;
    $("#update-go").disabled = false;
  }
}

async function checkUpdate() {
  if (updateBusy) return;
  let u;
  try {
    u = await invoke("app_update_check");
  } catch (e) {
    console.warn("update check:", e);
    return;
  }
  if (!u || !u.newer || u.latest === dismissedVersion || u.latest === installedVersion) return;
  if (u.automatic && u.install === "appimage") return installUpdate(u);
  if (u.automatic) {
    pendingUpdate = u;
    return showUpdate(u.latest, { title: `SixLauncher ${u.latest} disponível`, sub: `Você usa a ${u.current}.`, go: true });
  }
  showUpdate(u.latest, { title: `SixLauncher ${u.latest} disponível`, sub: `Você usa a ${u.current}. ${manualHint(u)}`, url: u.url });
}

// -- desktop notifications --------------------------------------------------------------------
//
// Every hour, and shortly after start, the core compares the race, the runtime builds and the
// newest release with what it saw last (crates/core/src/notify.rs). Each event becomes a desktop
// notification in the player's language. The switches live in config.toml ([notifications]).

const NOTIFY_EVERY_MS = 60 * 60 * 1000;
const NOTIFY_FIRST_MS = 30 * 1000;
const notification = window.__TAURI__.notification;

// ["A", "B", "C"] -> "A, B and C", in the player's language.
function nameList(list) {
  try {
    return new Intl.ListFormat(I18N.locale(), { style: "long", type: "conjunction" }).format(list);
  } catch {
    return list.join(", ");
  }
}

function describe(ev) {
  if (ev.kind === "overtake") {
    const vars = { name: ev.name, passed: nameList(ev.passed), score: pct(ev.score) };
    return ev.leader
      ? { title: t("notify.leaderTitle", vars), body: t("notify.leaderBody", vars) }
      : { title: t("notify.overtakeTitle", vars), body: t("notify.overtakeBody", vars) };
  }
  if (ev.kind === "runtime") {
    const vars = { name: BACKEND_NAME[ev.backend] || ev.backend, commit: ev.commit.slice(0, 7), repo: ev.repo || ev.effort || "?" };
    return { title: t("notify.runtimeTitle", vars), body: t("notify.runtimeBody", vars) };
  }
  if (ev.kind === "app") return { title: t("notify.appTitle", { version: ev.version }), body: t("notify.appBody", { current: ev.current }) };
  return null;
}

async function notifyAllowed() {
  if (!notification) return false;
  if (await notification.isPermissionGranted()) return true;
  return (await notification.requestPermission()) === "granted";
}

async function pollNotifications() {
  let events;
  try {
    events = await invoke("notifications_poll");
  } catch (e) {
    console.warn("notifications:", e);
    return;
  }
  if (!events.length || !(await notifyAllowed().catch(() => false))) return;
  for (const ev of events) {
    const n = describe(ev);
    if (n) notification.sendNotification(n);
  }
}

function showNotifySettings(s = {}) {
  for (const box of $$("[data-notify]")) box.checked = s[box.dataset.notify] !== false;
}

async function saveNotifySettings() {
  const settings = Object.fromEntries($$("[data-notify]").map((b) => [b.dataset.notify, b.checked]));
  try {
    config.notifications = await invoke("notifications_set", { settings });
    say("#notify-info", "notify.saved");
  } catch (e) {
    plain("#notify-info", String(e));
    showNotifySettings(config.notifications);
  }
}

async function init() {
  I18N.fillPicker($("#lang"));
  $("#lang").addEventListener("change", (ev) => I18N.set(ev.target.value));
  I18N.onChange(relabel);
  showPrompts();

  const state = await invoke("get_state");
  offerRunners(state.platform || "linux");
  config = state.config;
  fillForm();
  refreshTitle();
  setRunning(state.running);
  showVtKey(state.virustotal);
  say("#config-path", "config.path", { path: state.config_path });
  say("#log-path", "log.path", { path: state.log_file });
  showProtons(state.protons);
  $('[data-path="runner.path"]').addEventListener("change", () => showProtons(protonList));
  $("#proton-check").addEventListener("click", () =>
    guarded($("#proton-check"), async () => {
      say("#proton-info", "proton.loading");
      showReleases(await invoke("proton_available"));
      say("#proton-info", "proton.intro");
    }),
  );
  $("#proton-get").addEventListener("click", () => guarded($("#proton-get"), installProton));

  for (const b of $$(".tabs button")) b.addEventListener("click", () => selectTab(b.dataset.tab));
  for (const el of $$('select[data-path], input[data-kind="bool"]')) el.addEventListener("change", refreshVisibility);
  loadHardware();
  $('[data-path="game.dir"]').addEventListener("change", refreshTitle);
  $('[data-path="game.exe"]').addEventListener("change", verifyGame);
  $('[data-path="game.verify"]').addEventListener("change", verifyGame);
  $("#verify-zip").addEventListener("click", () => guarded($("#verify-zip"), verifyZip));
  $('[data-path="backend.exe"]').addEventListener("change", async (ev) => {
    const kind = await invoke("guess_backend", { exe: ev.target.value });
    if (kind) {
      $('[data-path="backend.kind"]').value = kind;
      refreshVisibility();
    }
    showBuilds(lastRuntimes);
  });

  for (const b of $$("[data-browse]")) {
    b.addEventListener("click", async () => {
      const input = $(`[data-path="${b.dataset.target}"]`);
      const directory = b.dataset.browse === "dir" || (b.dataset.browse === "any" && $('[data-path="runner.kind"]').value === "proton");
      const picked = await dialog.open({ directory, multiple: false, defaultPath: input.value || undefined });
      if (typeof picked === "string") {
        input.value = picked;
        input.dispatchEvent(new Event("change"));
      }
    });
  }

  $("#play").addEventListener("click", () => guarded($("#play"), () => play(false)));
  $("#banner-force").addEventListener("click", () => guarded($("#play"), () => play(true)));
  $("#banner-close").addEventListener("click", hideBanner);
  $("#stop").addEventListener("click", () => invoke("stop_game"));
  $("#save").addEventListener("click", () => guarded($("#save"), save));
  $("#setup").addEventListener("click", () =>
    guarded($("#setup"), async () => {
      await save();
      say("#save-status", "setup.busy");
      const lines = await invoke("setup_prefix", { config });
      for (const l of lines) appendLog(l, "stdout");
      if (lines.length) plain("#save-status", lines[lines.length - 1]);
      else say("#save-status", "setup.done");
    }),
  );
  $("#desktop").addEventListener("click", () =>
    guarded($("#desktop"), async () => {
      await save();
      const path = await invoke("create_desktop_entry", { config });
      say("#save-status", "desktop.done", { path });
    }),
  );
  $("#doctor").addEventListener("click", () =>
    guarded($("#doctor"), async () => {
      say("#doctor-status", "doctor.busy");
      showChecks(await invoke("run_doctor", { config: readForm() }));
    }),
  );
  $("#dry").addEventListener("click", () =>
    guarded($("#dry"), async () => {
      const out = $("#dry-out");
      out.hidden = false;
      out.textContent = await invoke("dry_run", { config: readForm() });
    }),
  );
  $("#log-clear").addEventListener("click", () => $("#log").replaceChildren());
  $("#fix-dialog").addEventListener("close", () => {
    const plan = pendingFix;
    pendingFix = null;
    if (plan && $("#fix-dialog").returnValue === "apply") guarded($("#doctor"), () => applyFix(plan));
  });
  $("#verbose-logs").checked = verboseOn(config);
  $("#verbose-logs").addEventListener("change", (ev) => guarded(ev.target, () => setVerbose(ev.target.checked)));
  $("#report-export").addEventListener("click", () => guarded($("#report-export"), exportReport));
  $("#vt-save").addEventListener("click", () =>
    guarded($("#vt-save"), async () => showVtKey(await invoke("virustotal_key", { key: $("#vt-key").value }))),
  );
  $("#vt-check").addEventListener("click", () =>
    guarded($("#vt-check"), async () => {
      say("#vt-info", "vt.checking");
      const list = await invoke("virustotal_check", { config: readForm() });
      showVerdicts(list);
      const flagged = list.filter((v) => v.state === "flagged").length;
      say("#vt-info", "vt.summary", { n: list.length, flagged });
    }),
  );
  $("#runtimes-check").addEventListener("click", () =>
    guarded($("#runtimes-check"), async () => showRuntimes(await invoke("runtime_status"))),
  );
  $("#backend-build").addEventListener("change", (ev) =>
    guarded(ev.target, () =>
      pickBuild(ev.target).catch((e) => {
        showBuilds(lastRuntimes); // back to the build actually in use
        throw e;
      }),
    ),
  );
  // The Backend list comes from the builder: load it once at start, without blocking.
  showBuilds(null);
  invoke("runtime_status")
    .then(showRuntimes)
    .catch((e) => {
      showBuilds({ backends: [] });
      console.warn("runtimes:", e);
      say("#runtimes-info", "runtimes.offline");
    });
  showNotifySettings(config.notifications);
  for (const box of $$("[data-notify]")) box.addEventListener("change", saveNotifySettings);
  $("#notify-test").addEventListener("click", () =>
    guarded($("#notify-test"), async () => {
      if (!(await notifyAllowed())) return say("#notify-info", "notify.denied");
      notification.sendNotification({ title: t("notify.testTitle"), body: t("notify.testBody") });
      say("#notify-info", "notify.testSent");
    }),
  );
  $("#status-refresh").addEventListener("click", () =>
    guarded($("#status-refresh"), async () => {
      say("#status-info", "status.loading");
      showEfforts(await invoke("portwatch_status"));
    }),
  );

  // Console-style shortcuts, shown in the footer.
  const tabs = ["game", "compat", "doctor", "log", "status"];
  document.addEventListener("keydown", (ev) => {
    if (!ev.ctrlKey || ev.altKey || ev.metaKey) return;
    if (ev.key === "Enter" && !$("#play").hidden) $("#play").click();
    else if (ev.key.toLowerCase() === "d") {
      selectTab("doctor");
      $("#doctor").click();
    } else if (ev.key >= "1" && ev.key <= "5") selectTab(tabs[Number(ev.key) - 1]);
    else return;
    ev.preventDefault();
  });

  await listen("runtime-progress", (ev) => {
    const { backend, done, total } = ev.payload;
    const pct = total ? Math.floor((done * 100) / total) : 0;
    say("#runtimes-info", "runtimes.progress", { name: BACKEND_NAME[backend], pct, mb: (total / 1048576).toFixed(1) });
  });
  await listen("proton-progress", (ev) => {
    const { name, done, total } = ev.payload;
    const pct = total ? Math.floor((done * 100) / total) : 0;
    say("#proton-info", "proton.progress", { name, pct, mb: (total / 1048576).toFixed(1) });
  });
  await listen("app-update-progress", (ev) => {
    const { done, total } = ev.payload;
    const mb = (n) => (n / 1048576).toFixed(1);
    $("#update-sub").textContent = total ? `${Math.floor((done * 100) / total)}% de ${mb(total)} MB` : `${mb(done)} MB`;
  });
  $("#update-go").addEventListener("click", () => pendingUpdate && installUpdate(pendingUpdate));
  $("#update-restart").addEventListener("click", async () => {
    try {
      await invoke("app_restart");
    } catch (e) {
      $("#update-sub").textContent = String(e);
    }
  });
  $("#update-close").addEventListener("click", () => {
    if (updateBusy) return;
    dismissedVersion = shownVersion;
    $("#update").hidden = true;
  });
  window.__TAURI__.app.getVersion().then((v) => ($("#app-version").textContent = `SixLauncher ${v} ·`)).catch(() => {});
  checkUpdate();
  setInterval(checkUpdate, UPDATE_EVERY_MS);
  setTimeout(pollNotifications, NOTIFY_FIRST_MS);
  setInterval(pollNotifications, NOTIFY_EVERY_MS);

  await listen("game-log", (ev) => appendLog(ev.payload.line, ev.payload.stream));
  await listen("game-exit", (ev) => {
    setRunning(false);
    appendLog(`— ${t("log.exit", { code: ev.payload ?? t("log.unknownCode") })} —`, "stdout");
  });
}

init().catch((e) => showBanner(String(e), false));
