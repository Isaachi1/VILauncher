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
  $("#backend-label").textContent = `${backend === "xcl" ? "XCL" : "VIctory"} · ${runner === "wine" ? "Wine + vkd3d-proton" : "Proton"}`;
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

function stageClass(gta) {
  if (gta.playable > 0) return "ok";
  return gta.world > 0 || gta.menu > 0 ? "warn" : "fail";
}

function showEfforts(report) {
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
    title.append(el("h3", null, e.name));
    title.append(el("p", "repo", e.repo ? `github.com/${e.repo}` : "Código fechado: ninguém fora do projeto consegue rodar"));
    head.append(title);
    const chips = el("div", "chips");
    if (e.backend) chips.append(el("span", "pill violet", `backend ${BACKEND_NAME[e.backend]}`));
    chips.append(el("span", `pill ${stageClass(e.gta)}`, e.partial ? `${e.stage} (em parte)` : e.stage));
    if (e.gta.fps > 0) chips.append(el("span", "pill gold", `~${e.gta.fps} fps`));
    head.append(chips);
    body.append(head, milestoneBar(e.gta));
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

// The tracker's steps as one segmented bar: a full segment per step reached, half for "in part".
const STEPS = [["loads", "Carrega"], ["boots", "Inicia"], ["menu", "Menus"], ["world", "No mundo"], ["playable", "Jogável"], ["story", "História"]];

function milestoneBar(gta) {
  const wrap = el("div", "milestones");
  const bar = el("div", "bar");
  const legend = el("div", "legend");
  for (const [key, label] of STEPS) {
    const v = Math.max(0, Math.min(1, gta[key] || 0));
    const seg = el("span", "seg");
    const fill = el("i");
    fill.style.width = `${v * 100}%`;
    seg.append(fill);
    bar.append(seg);
    legend.append(el("span", v > 0 ? "hit" : null, label));
  }
  wrap.append(bar, legend);
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

function showRuntimes(rt) {
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
