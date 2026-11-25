// VILauncher's window. Everything happens in Rust (vilauncher-core); this file
// only edits the configuration and shows what comes back.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const dialog = window.__TAURI__.dialog;

const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => [...document.querySelectorAll(sel)];

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
    else el.value = v ?? "";
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
  $("#runner-path-label").textContent = runner === "wine" ? "Binário do wine (vazio: o do PATH)" : "Pasta do Proton (vazio: o melhor encontrado)";
  const build = runtimes && runtimes.backends.find((r) => r.backend === backend);
  const from = build && (build.source || build.latest);
  const name = from && from.repo ? from.repo : BACKEND_NAME[backend] || backend;
  $("#backend-label").textContent = `${name} · ${runner === "wine" ? "Wine + vkd3d-proton" : "Proton"}`;
}

async function refreshTitle() {
  const dir = $('[data-path="game.dir"]').value.trim();
  if (!dir) {
    $("#game-title").textContent = "Nenhum jogo escolhido";
    $("#game-sub").textContent = "Escolha a pasta desempacotada de um jogo seu na aba Jogo.";
    return;
  }
  try {
    const info = await invoke("game_info", { dir });
    $("#game-title").textContent = info.display_name || info.identity || dir.split("/").pop();
    $("#game-sub").textContent = [info.identity, info.version].filter(Boolean).join(" · ") || dir;
  } catch (e) {
    $("#game-title").textContent = dir.split("/").pop() || dir;
    $("#game-sub").textContent = String(e);
  }
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
  $("#live-text").textContent = running ? "Rodando" : "Pronto";
}

const STATUS_TEXT = { ok: "OK", warn: "AVISO", fail: "FALHA" };

function showChecks(checks) {
  const body = $("#checks tbody");
  body.replaceChildren();
  for (const c of checks) {
    const tr = document.createElement("tr");
    const pill = document.createElement("span");
    pill.className = `pill ${c.status}`;
    pill.textContent = STATUS_TEXT[c.status];
    const tdS = document.createElement("td");
    tdS.appendChild(pill);
    const tdN = document.createElement("td");
    tdN.className = "name";
    tdN.textContent = c.name;
    const tdD = document.createElement("td");
    tdD.textContent = c.detail;
    tr.append(tdS, tdN, tdD);
    body.appendChild(tr);
  }
  const fails = checks.filter((c) => c.status === "fail").length;
  const warns = checks.filter((c) => c.status === "warn").length;
  $("#doctor-status").textContent = `${fails} falha(s), ${warns} aviso(s), ${checks.length - fails - warns} ok`;
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

const pct = (v) => (Math.round(v * 10) / 10).toLocaleString("pt-BR");
let milestones = [];
const msLabel = (key) => (milestones.find((m) => m[0] === key) || [key, key])[1];
const TRACK_NAMES = "História · Gráficos · Desempenho · Estabilidade";

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
    line.append("Líder: ", el("b", null, leader.name), " ");
    if (leader.repo) line.append(`de ${leader.repo.split("/")[0]}. `);
    else if (leader.based_on.length) {
      const priv = el("span", "ov-private");
      priv.append(lockIcon(), "versão privada baseada em ", el("b", null, leader.based_on.join(", ")));
      line.append(priv, ". ");
    }
    line.append(leader.reached === "groundwork" ? "Ainda na preparação." : `Chegou a “${msLabel(leader.reached)}”.`);
  } else line.textContent = "Nenhum projeto acompanhado.";

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
    label.append(el("b", null, `${m[2]}%`), el("span", "dot"), m[1]);
    labels.append(label);
  });
  labels.append(el("p", "mega-zone", TRACK_NAMES));

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

function showEfforts(report) {
  lastReport = report;
  showOverall(report);
  loadAvatars(report).catch(() => {});
  const box = $("#efforts");
  box.replaceChildren();
  const when = report.updated ? new Date(report.updated).toLocaleString("pt-BR") : "?";
  $("#status-info").textContent = report.stale
    ? `Sem conexão; mostrando a cópia salva (atualizada em ${when}). ${report.stale}`
    : `Atualizado pelo PortWatch VI em ${when}`;
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
    names.append(el("p", "repo", e.repo ? `github.com/${e.repo}` : e.based_on.length ? `Versão privada baseada em ${e.based_on.join(", ")}` : "Código fechado: ninguém fora do projeto consegue rodar"));
    who.append(avatarFace(e.id, e.name, "e-avatar"), names);
    title.append(who);
    const chips = el("div", "chips");
    if (e.backend) chips.append(el("span", "pill violet", `backend ${BACKEND_NAME[e.backend]}`));
    if (e.partial) chips.append(el("span", "pill warn", `${e.stage} em parte`));
    if (e.gta.fps > 0) chips.append(el("span", "pill gold", `~${e.gta.fps} fps`));
    if (watch) chips.append(el("span", "pill", "Em observação"));
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
  milestones.slice(0, Math.max(1, reached + 1)).forEach((m) => legend.append(el("span", "hit", m[1])));
  wrap.append(track, legend);
  return wrap;
}

// -- runtime updates ---------------------------------------------------------------------------

const short = (c) => (c ? c.slice(0, 10) : "—");

const SCAN_TEXT = { clean: "VirusTotal: limpo", flagged: "VirusTotal: ALERTA", pending: "VirusTotal: em análise", skipped: "VirusTotal: não verificado", error: "VirusTotal: falhou" };
const SCAN_PILL = { clean: "ok", flagged: "fail" };

function sourceLine(a) {
  if (!a) return "";
  return [a.effort, a.repo ? `github.com/${a.repo}` : null, short(a.commit)].filter(Boolean).join(" · ");
}

// The Backend field lists the builds the CI publishes, by repository (one per family: the
// most advanced public effort on PortWatch VI), the recommended one first. The other
// efforts of the same families are listed too, disabled, with why the CI left them out.
let runtimes = null;

const MANAGED = "/vilauncher/runtimes/";

function buildLabel(r) {
  const a = r.latest || r.source;
  const family = BACKEND_NAME[r.backend];
  if (!a || !a.repo) return `${family} (nenhuma build publicada)`;
  const effort = a.effort && a.effort.toLowerCase() !== a.repo.split("/").pop().toLowerCase() ? ` · ${a.effort}` : "";
  return `${a.repo}${effort} (${family})`;
}

function fillBackendPicker(rt) {
  const sel = $('[data-path="backend.kind"]');
  const exe = $('[data-path="backend.exe"]');
  let keep = sel.value;
  // First run: nothing chosen yet, so start on the most advanced build.
  if (!exe.value.trim() && rt.recommended) keep = rt.recommended;
  const order = [...rt.backends].sort((a, b) => (b.backend === rt.recommended) - (a.backend === rt.recommended));
  const built = document.createElement("optgroup");
  built.label = "Builds do CI (PortWatch VI + VirusTotal)";
  for (const r of order) {
    const best = r.backend === rt.recommended ? " · mais avançado" : "";
    const state = r.exe ? "" : r.latest ? " · não instalada" : "";
    built.append(new Option(`${buildLabel(r)}${best}${state}`, r.backend));
  }
  const others = document.createElement("optgroup");
  others.label = "Outros projetos no PortWatch VI (sem build publicada)";
  for (const c of rt.candidates) {
    if (!c.family || !BACKEND_NAME[c.family] || c.result.startsWith("selected")) continue;
    const why = c.result.startsWith("behind ") ? `atrás de ${c.result.slice(7)}` : c.result;
    const name = c.name && c.name.toLowerCase() !== c.repo.split("/").pop().toLowerCase() ? ` · ${c.name}` : "";
    const opt = new Option(`${c.repo}${name} (${BACKEND_NAME[c.family]}) · ${why}`, "");
    opt.disabled = true;
    others.append(opt);
  }
  sel.replaceChildren(built);
  if (others.children.length) sel.append(others);
  sel.value = keep;
  if (sel.value !== keep) sel.value = order[0] ? order[0].backend : "victory";
  pickBackend();
}

// After the Backend field changes: use that family's installed build, unless the
// executable was set by hand to something the launcher did not install.
function pickBackend() {
  refreshVisibility();
  if (!runtimes) return;
  const kind = $('[data-path="backend.kind"]').value;
  const r = runtimes.backends.find((b) => b.backend === kind);
  if (!r) return;
  const exe = $('[data-path="backend.exe"]');
  const current = exe.value.trim();
  const managed = !current || current.includes(MANAGED);
  if (r.exe && r.exe !== current && managed) exe.value = r.exe;
  else if (!r.exe && managed) exe.value = "";
  const info = $("#runtimes-info");
  if (!r.exe && r.latest) info.textContent = `A build de ${buildLabel(r)} ainda não está instalada: clique em Instalar abaixo.`;
  else if (r.exe && exe.value.trim() === r.exe) info.textContent = `Usando a build de ${buildLabel(r)}.`;
}

function showRuntimes(rt) {
  runtimes = rt;
  fillBackendPicker(rt);
  const body = $("#runtimes tbody");
  body.replaceChildren();
  const current = $('[data-path="backend.exe"]').value.trim();
  const best = rt.backends.find((r) => r.backend === rt.recommended);
  const pick = $("#runtimes-best");
  pick.hidden = !best || !best.latest;
  if (best && best.latest) {
    $("#runtimes-best-text").textContent = `Mais avançado com código público no PortWatch VI: ${sourceLine(best.latest)} (${BACKEND_NAME[best.backend]})`;
    const b = $("#runtimes-best-go");
    b.onclick = () => guarded(b, () => (best.update ? installRuntime(best.backend, b) : useRuntime(best.backend, best.exe)));
    b.textContent = best.update ? "Instalar e usar" : "Usar";
  }
  const cands = $("#runtimes-candidates");
  cands.replaceChildren();
  for (const c of rt.candidates) cands.append(el("li", null, `${c.name} (${c.repo}): ${c.result}`));
  $("#runtimes-why").hidden = rt.candidates.length === 0;
  for (const r of rt.backends) {
    const tr = el("tr");
    tr.append(el("td", "name", BACKEND_NAME[r.backend]));
    const info = el("td");
    const latest = r.latest ? `${sourceLine(r.latest)} (${(r.latest.size / 1048576).toFixed(1)} MB)` : "nenhuma publicada";
    info.append(el("div", null, `Mais nova: ${latest}`));
    info.append(el("div", "status", `Instalada: ${r.installed ? sourceLine(r.source) || short(r.installed) : "nenhuma"}`));
    const scan = r.latest && r.latest.virustotal;
    if (scan) {
      const pill = el("span", `pill ${SCAN_PILL[scan.verdict] || "warn"}`, SCAN_TEXT[scan.verdict] || scan.verdict);
      const flagged = scan.files.filter((f) => f.malicious > 0).map((f) => `${f.name}: ${f.malicious}/${f.engines}`);
      const line = el("div", null);
      line.append(pill, " ", flagged.length ? flagged.join(", ") : scan.files.map((f) => `${f.name} ${f.malicious}/${f.engines}`).join(" · "));
      info.append(line);
    }
    tr.append(info);
    const td = el("td");
    if (r.latest && r.update) {
      const b = el("button", "primary", r.installed ? "Atualizar" : "Instalar");
      b.addEventListener("click", () => guarded(b, () => installRuntime(r.backend, b)));
      td.append(b);
    } else if (r.exe && r.exe !== current) {
      const b = el("button", null, "Usar esta build");
      b.addEventListener("click", () => useRuntime(r.backend, r.exe));
      td.append(b);
    } else if (r.exe) {
      td.append(el("span", "pill ok", "em uso"));
    }
    tr.append(td);
    body.append(tr);
  }
}

function useRuntime(backend, exe) {
  $('[data-path="backend.kind"]').value = backend;
  $('[data-path="backend.exe"]').value = exe;
  refreshVisibility();
  return save().then(() => $("#runtimes-check").click());
}

async function installRuntime(backend, button, force = false) {
  $("#runtimes-info").textContent = `Baixando ${BACKEND_NAME[backend]}…`;
  const r = await invoke("install_runtime", { backend, force });
  showVerdicts(r.verdicts);
  if (r.blocked) {
    $("#runtimes-info").textContent = `${BACKEND_NAME[backend]} não foi instalado.`;
    showBanner(`${r.blocked}\n\nA build instalada continua a mesma.`, false);
    const again = el("button", "danger", "Instalar mesmo assim");
    again.addEventListener("click", () => guarded(again, () => installRuntime(backend, again, true)));
    button.replaceWith(again);
    return;
  }
  const vt = r.checked ? "conferido no VirusTotal" : "sem VirusTotal (nenhuma chave salva)";
  $("#runtimes-info").textContent = `${BACKEND_NAME[backend]} instalado em ${r.exe} (${vt})`;
  button.hidden = true;
  await useRuntime(backend, r.exe);
}

// -- VirusTotal ----------------------------------------------------------------------------------

const VT_TEXT = { clean: "LIMPO", flagged: "ALERTA", unknown: "DESCONHECIDO" };
const VT_PILL = { clean: "ok", flagged: "fail", unknown: "warn" };

function showVerdicts(list) {
  const body = $("#vt tbody");
  body.replaceChildren();
  for (const v of list) {
    const tr = el("tr");
    const pill = el("span", `pill ${VT_PILL[v.state]}`, VT_TEXT[v.state]);
    const tdS = el("td");
    tdS.append(pill);
    tr.append(tdS, el("td", "name", v.file));
    let detail;
    if (v.state === "unknown") detail = "O VirusTotal ainda não conhece este arquivo.";
    else detail = `${v.malicious} de ${v.engines} antivírus acusam; ${v.suspicious} suspeito(s). ${v.link}`;
    const tdD = el("td", null, detail);
    if (v.state === "unknown") {
      const up = el("button", "ghost", "Enviar para análise");
      up.title = "O arquivo é enviado ao VirusTotal e fica disponível para ele e seus parceiros.";
      up.addEventListener("click", () =>
        guarded(up, async () => {
          await invoke("virustotal_upload", { config: readForm(), file: v.file });
          up.replaceWith(el("span", "status", " Enviado; verifique de novo em alguns minutos."));
        }),
      );
      tdD.append(" ", up);
    }
    tr.append(tdD);
    body.append(tr);
  }
}

function showVtKey(saved) {
  $("#vt-key").value = "";
  $("#vt-key").placeholder = saved ? "Chave salva (digite outra para trocar, ou salve vazio para apagar)" : "Chave da API do VirusTotal";
  $("#vt-check").disabled = !saved;
  $("#vt-info").textContent = saved
    ? "Só o SHA-256 dos .exe e .dll do backend é consultado; nenhum arquivo é enviado sem você pedir."
    : "Sem chave, as builds não são conferidas no VirusTotal. Uma conta gratuita do VirusTotal tem uma.";
}

// -- actions -----------------------------------------------------------------------------

async function save() {
  config = readForm();
  const path = await invoke("save_config", { config });
  $("#save-status").textContent = `Salvo em ${path}`;
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
    showBanner(`Não dá para rodar ainda:\n${e}`, !force);
  }
}

async function init() {
  const state = await invoke("get_state");
  config = state.config;
  fillForm();
  refreshTitle();
  setRunning(state.running);
  showVtKey(state.virustotal);
  $("#config-path").textContent = `Configuração: ${state.config_path}`;
  $("#log-path").textContent = `Também gravado em ${state.log_file}`;
  const list = $("#protons");
  list.replaceChildren();
  for (const p of state.protons) {
    const opt = document.createElement("option");
    opt.value = p.path;
    opt.label = p.version;
    list.appendChild(opt);
  }

  for (const b of $$(".tabs button")) b.addEventListener("click", () => selectTab(b.dataset.tab));
  for (const el of $$('select[data-path]')) el.addEventListener("change", refreshVisibility);
  $('[data-path="backend.kind"]').addEventListener("change", pickBackend);
  $('[data-path="game.dir"]').addEventListener("change", refreshTitle);
  $('[data-path="backend.exe"]').addEventListener("change", async (ev) => {
    const kind = await invoke("guess_backend", { exe: ev.target.value });
    if (kind) {
      $('[data-path="backend.kind"]').value = kind;
      refreshVisibility();
    }
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
      $("#save-status").textContent = "Preparando o prefixo…";
      const lines = await invoke("setup_prefix", { config });
      for (const l of lines) appendLog(l, "stdout");
      $("#save-status").textContent = lines.length ? lines[lines.length - 1] : "Prefixo pronto";
    }),
  );
  $("#desktop").addEventListener("click", () =>
    guarded($("#desktop"), async () => {
      await save();
      const path = await invoke("create_desktop_entry", { config });
      $("#save-status").textContent = `Atalho criado em ${path}`;
    }),
  );
  $("#doctor").addEventListener("click", () =>
    guarded($("#doctor"), async () => {
      $("#doctor-status").textContent = "Verificando…";
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
  $("#vt-save").addEventListener("click", () =>
    guarded($("#vt-save"), async () => showVtKey(await invoke("virustotal_key", { key: $("#vt-key").value }))),
  );
  $("#vt-check").addEventListener("click", () =>
    guarded($("#vt-check"), async () => {
      $("#vt-info").textContent = "Consultando o VirusTotal…";
      const list = await invoke("virustotal_check", { config: readForm() });
      showVerdicts(list);
      const flagged = list.filter((v) => v.state === "flagged").length;
      $("#vt-info").textContent = `${list.length} arquivo(s) conferido(s); ${flagged} com alerta.`;
    }),
  );
  $("#runtimes-check").addEventListener("click", () =>
    guarded($("#runtimes-check"), async () => showRuntimes(await invoke("runtime_status"))),
  );
  // Fill the Backend field with the published builds right away; offline it keeps the two
  // families until "Procurar builds novas" works.
  invoke("runtime_status").then(showRuntimes, () => {});
  $("#status-refresh").addEventListener("click", () =>
    guarded($("#status-refresh"), async () => {
      $("#status-info").textContent = "Buscando…";
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
    $("#runtimes-info").textContent = `Baixando ${BACKEND_NAME[backend]}: ${pct}% de ${(total / 1048576).toFixed(1)} MB`;
  });
  await listen("game-log", (ev) => appendLog(ev.payload.line, ev.payload.stream));
  await listen("game-exit", (ev) => {
    setRunning(false);
    appendLog(`— o jogo terminou (código ${ev.payload ?? "desconhecido"}) —`, "stdout");
  });
}

init().catch((e) => showBanner(String(e), false));
