// GenshinDamageLens overlay: renders session stats pushed by the capture backend.

const tauri = window.__TAURI__ || null;
const invoke = tauri ? tauri.core.invoke : null;
const listen = tauri ? tauri.event.listen : null;

const $ = (id) => document.getElementById(id);
const panel = $("panel");

const ELEMENTS = ["pyro", "hydro", "cryo", "electro", "dendro", "anemo", "geo", "physical"];
const FEED_ROWS = 7;
const SPARK_SAMPLES = 120; // stats arrive four times a second: 30 s of history
const STORE_KEY = "gdl.panel";

const view = {
  passthrough: false,
  paused: false,
  status: "unavailable",
  demo: null,
  spark: [],
  seenHits: new Set(),
  lastHits: [],
};

// ── Formatting ────────────────────────────────────────────────────────
const whole = (n) => Math.round(n).toLocaleString("en-US");

function compact(n) {
  if (n >= 1e9) return (n / 1e9).toFixed(2) + "B";
  if (n >= 1e6) return (n / 1e6).toFixed(2) + "M";
  if (n >= 1e4) return (n / 1e3).toFixed(1) + "K";
  return whole(n);
}

function age(ms) {
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m`;
}

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

// ── Rendering ─────────────────────────────────────────────────────────
function renderStats(stats) {
  $("dps-display").textContent = whole(stats.dps);
  panel.dataset.idle = String(stats.dps === 0);
  $("total-damage-display").textContent = compact(stats.total_damage);
  $("avg-dps-display").textContent = compact(stats.avg_dps);
  $("total-hits-display").textContent = whole(stats.total_hits);
  $("crit-rate-display").textContent = `${Math.round(stats.crit_rate_pct)}%`;

  const hasPeak = stats.peak_hit > 0;
  $("peak-display").textContent = hasPeak ? whole(stats.peak_hit) : "–";
  const badge = $("peak-element-badge");
  badge.hidden = !hasPeak;
  badge.className = `chip ${stats.peak_element}`;
  badge.textContent = stats.peak_element;
  $("peak-dps-display").textContent = stats.peak_dps > 0 ? whole(stats.peak_dps) : "–";

  renderElements(stats.elemental_breakdown || {}, stats.total_damage);
  renderFeed(stats.recent_hits || []);
  renderSpark(stats.dps);
}

function renderElements(breakdown, total) {
  const bar = $("elemental-bar");
  const legend = $("element-legend");
  const rows = Object.entries(breakdown)
    .filter(([, amount]) => amount > 0)
    .sort((a, b) => b[1] - a[1]);
  bar.replaceChildren();
  legend.replaceChildren();
  if (!rows.length || !total) {
    legend.append(el("div", "empty", "No damage recorded yet."));
    return;
  }
  for (const [element, amount] of rows) {
    const share = amount / total;
    const seg = el("div", `stack-seg ${element}`);
    seg.style.flexGrow = String(amount);
    seg.title = `${element}: ${whole(amount)}`;
    bar.append(seg);

    const row = el("div", `legend-row ${element}`);
    row.title = whole(amount);
    row.append(
      el("span", "dot"),
      el("span", "name", element),
      el("span", "pct", share >= 0.995 ? "100%" : `${(share * 100).toFixed(share < 0.1 ? 1 : 0)}%`),
    );
    legend.append(row);
  }
}

function renderFeed(hits) {
  view.lastHits = hits;
  const list = $("hit-ticker-list");
  if (!hits.length) {
    list.replaceChildren(
      el("div", "empty", "Hits appear here about half a second after the number shows in game."),
    );
    $("feed-count").textContent = "";
    view.seenHits.clear();
    return;
  }
  const rows = hits.slice(0, FEED_ROWS).map((hit) => {
    const row = el("div", `hit ${hit.element}`);
    if (!view.seenHits.has(hit.id)) row.classList.add("fresh");
    row.append(el("span", "dot"), el("span", "name", hit.element));
    if (hit.is_crit) row.append(el("span", "crit", "CRIT"));
    row.append(el("span", "value", whole(hit.value)), el("span", "age", age(hit.timestamp_ms)));
    return row;
  });
  view.seenHits = new Set(hits.map((hit) => hit.id));
  list.replaceChildren(...rows);
  $("feed-count").textContent = `last ${rows.length}`;
}

function renderSpark(dps) {
  view.spark.push(dps);
  if (view.spark.length > SPARK_SAMPLES) view.spark.shift();
  const max = Math.max(...view.spark);
  if (max <= 0) {
    $("spark-line").setAttribute("d", "");
    $("spark-area").setAttribute("d", "");
    return;
  }
  const step = 120 / (SPARK_SAMPLES - 1);
  const offset = SPARK_SAMPLES - view.spark.length;
  const points = view.spark.map((value, i) => {
    const x = ((offset + i) * step).toFixed(1);
    const y = (38 - (value / max) * 34).toFixed(1);
    return `${x},${y}`;
  });
  const line = `M${points.join("L")}`;
  $("spark-line").setAttribute("d", line);
  $("spark-area").setAttribute("d", `${line}L120,40L${(offset * step).toFixed(1)},40Z`);
}

function renderStatus(status) {
  view.status = status;
  panel.dataset.status = status;
  const text = { tracking: "Tracking", paused: "Paused", unavailable: "Waiting for game" };
  $("status-text").textContent = text[status] || "Waiting for game";
  view.paused = status === "paused";
  $("icon-pause").toggleAttribute("hidden", view.paused);
  $("icon-play").toggleAttribute("hidden", !view.paused);
  $("btn-pause").title = view.paused ? "Resume capture" : "Pause capture";
  if (status !== "tracking") {
    $("capture-details").textContent =
      status === "paused" ? "Capture paused" : "Genshin Impact window not found";
  }
}

function renderDiagnostics(d) {
  if (d.status !== "tracking") return;
  $("status-text").textContent = `Tracking · ${Math.round(d.fps)} fps`;
  const capture = Number.isFinite(d.capture_ms) ? d.capture_ms : 0;
  $("capture-details").textContent =
    `${d.width}×${d.height} · read ${d.processing_ms.toFixed(1)} ms · ` +
    `capture ${capture.toFixed(1)} ms · ${d.hits} hits`;
  $("capture-details").title =
    `Per frame: ${d.processing_ms.toFixed(1)} ms reading numbers, ${capture.toFixed(1)} ms ` +
    `copying the screen. ${d.candidates} single-frame readings became ${d.hits} confirmed ` +
    "hits in the last two seconds. Frame rate is in the status line above.";
}

function renderPassthrough(on) {
  view.passthrough = on;
  document.body.classList.toggle("passthrough", on);
  closeMenu();
}

// ── Actions ───────────────────────────────────────────────────────────
async function setPassthrough(on) {
  renderPassthrough(on);
  if (!invoke) return;
  try {
    await invoke("toggle_click_through", { ignore: on });
  } catch (error) {
    renderPassthrough(!on);
    console.warn("Could not change click-through:", error);
  }
}

async function togglePause() {
  const resume = view.paused;
  if (invoke) {
    await invoke("set_capture_active", { enabled: resume });
  } else {
    renderStatus(resume ? "tracking" : "paused");
  }
}

async function reset() {
  view.spark = [];
  view.seenHits.clear();
  if (invoke) {
    renderStats(await invoke("reset_stats"));
  } else {
    mock.reset();
  }
}

async function testHit() {
  if (invoke) {
    await invoke("simulate_hit", { element: null, isCrit: null });
  } else {
    mock.hit();
  }
}

function toggleDemo() {
  const button = $("btn-auto-sim");
  if (view.demo) {
    clearInterval(view.demo);
    view.demo = null;
  } else {
    view.demo = setInterval(testHit, 450);
  }
  button.classList.toggle("on", Boolean(view.demo));
}

// Recorders only see the overlay when it is not excluded from screen capture.
// The capture backend then blanks the panel's box so it never reads itself.
async function setShowInRecordings(on) {
  if (invoke) {
    try {
      await invoke("set_show_in_recordings", { visible: on });
    } catch (error) {
      console.warn("Could not change recording visibility:", error);
      return;
    }
  }
  $("btn-recordings").classList.toggle("on", on);
  savePlacement({ recordings: on });
}

// Tell the backend where the panel is, in physical pixels of the window.
function reportPanelRect() {
  if (!invoke) return;
  const box = panel.getBoundingClientRect();
  const scale = window.devicePixelRatio || 1;
  invoke("set_panel_rect", {
    x: box.left * scale,
    y: box.top * scale,
    width: box.width * scale,
    height: box.height * scale,
  }).catch((error) => console.warn("Could not report panel position:", error));
}

function closeMenu() {
  $("menu").hidden = true;
  $("btn-more").setAttribute("aria-expanded", "false");
}

function toggleMenu() {
  const open = $("menu").hidden;
  $("menu").hidden = !open;
  $("btn-more").setAttribute("aria-expanded", String(open));
}

function setCompact(on) {
  document.body.classList.toggle("compact", on);
  $("btn-compact").classList.toggle("on", on);
  $("btn-compact").title = on ? "Full view" : "Compact view";
  savePlacement({ compact: on });
}

// ── Placement: drag by the title bar, remembered between runs ─────────
function loadPlacement() {
  try {
    return JSON.parse(localStorage.getItem(STORE_KEY)) || {};
  } catch {
    return {};
  }
}

function savePlacement(change) {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify({ ...loadPlacement(), ...change }));
  } catch {
    // Placement is a convenience; the default spot still works.
  }
}

function placePanel(left, top) {
  const maxLeft = Math.max(0, window.innerWidth - panel.offsetWidth);
  const maxTop = Math.max(0, window.innerHeight - 60);
  panel.style.left = `${Math.min(Math.max(0, left), maxLeft)}px`;
  panel.style.top = `${Math.min(Math.max(0, top), maxTop)}px`;
  reportPanelRect();
}

function setupDrag() {
  const handle = $("drag-handle");
  let grab = null;
  handle.addEventListener("pointerdown", (event) => {
    if (view.passthrough || event.button !== 0 || event.target.closest("button, .menu")) return;
    const box = panel.getBoundingClientRect();
    grab = { dx: event.clientX - box.left, dy: event.clientY - box.top };
    handle.setPointerCapture(event.pointerId);
  });
  handle.addEventListener("pointermove", (event) => {
    if (grab) placePanel(event.clientX - grab.dx, event.clientY - grab.dy);
  });
  const release = () => {
    if (!grab) return;
    grab = null;
    const box = panel.getBoundingClientRect();
    savePlacement({ left: Math.round(box.left), top: Math.round(box.top) });
  };
  handle.addEventListener("pointerup", release);
  handle.addEventListener("pointercancel", release);
}

function recenter() {
  panel.style.left = "";
  panel.style.top = "";
  reportPanelRect();
  savePlacement({ left: undefined, top: undefined });
  closeMenu();
}

// ── Browser preview without the backend ───────────────────────────────
const mock = {
  hits: [],
  stats: null,
  reset() {
    this.hits = [];
    this.stats = {
      dps: 0, peak_dps: 0, avg_dps: 0, combat_secs: 0, peak_hit: 0, peak_element: "physical",
      total_damage: 0, total_hits: 0, crit_hits: 0, crit_rate_pct: 0,
      elemental_breakdown: {}, recent_hits: [],
    };
    renderStats(this.stats);
  },
  hit() {
    const s = this.stats;
    const element = ELEMENTS[Math.floor(Math.random() ** 2 * ELEMENTS.length)];
    const is_crit = Math.random() < 0.6;
    const value = Math.floor(is_crit ? 9000 + Math.random() * 30000 : 1500 + Math.random() * 6000);
    const now = Date.now();
    const last = this.hits[0];
    s.combat_secs += last && now - last.timestamp_ms <= 5000 ? (now - last.timestamp_ms) / 1000 : 1;
    this.hits.unshift({ id: now + Math.random(), timestamp_ms: now, value, element, is_crit });
    this.hits.length = Math.min(this.hits.length, 200);
    s.total_damage += value;
    s.total_hits += 1;
    s.crit_hits += is_crit ? 1 : 0;
    s.crit_rate_pct = (s.crit_hits / s.total_hits) * 100;
    s.avg_dps = Math.round(s.total_damage / Math.max(1, s.combat_secs));
    if (value > s.peak_hit) [s.peak_hit, s.peak_element] = [value, element];
    s.elemental_breakdown[element] = (s.elemental_breakdown[element] || 0) + value;
    this.tick();
  },
  tick() {
    const s = this.stats;
    const cutoff = Date.now() - 5000;
    const windowDamage = this.hits
      .filter((hit) => hit.timestamp_ms >= cutoff)
      .reduce((sum, hit) => sum + hit.value, 0);
    s.dps = Math.round(windowDamage / 5);
    s.peak_dps = Math.max(s.peak_dps, s.dps);
    s.recent_hits = this.hits.slice(0, 15);
    renderStats(s);
  },
};

// ── Startup ───────────────────────────────────────────────────────────
async function start() {
  const placement = loadPlacement();
  if (Number.isFinite(placement.left) && Number.isFinite(placement.top)) {
    placePanel(placement.left, placement.top);
  }
  setCompact(Boolean(placement.compact));
  setupDrag();

  $("btn-clickthrough").addEventListener("click", () => setPassthrough(true));
  $("btn-pause").addEventListener("click", togglePause);
  $("btn-reset").addEventListener("click", reset);
  $("btn-compact").addEventListener("click", () =>
    setCompact(!document.body.classList.contains("compact")),
  );
  $("btn-more").addEventListener("click", toggleMenu);
  $("btn-simulate").addEventListener("click", testHit);
  $("btn-auto-sim").addEventListener("click", toggleDemo);
  $("btn-recenter").addEventListener("click", recenter);
  $("btn-recordings").addEventListener("click", () =>
    setShowInRecordings(!$("btn-recordings").classList.contains("on")),
  );
  // The panel grows and shrinks with its content and the compact view.
  new ResizeObserver(reportPanelRect).observe(panel);
  window.addEventListener("resize", reportPanelRect);
  reportPanelRect();
  if (placement.recordings) await setShowInRecordings(true);
  document.addEventListener("pointerdown", (event) => {
    if (!event.target.closest("#menu, #btn-more")) closeMenu();
  });

  if (listen) {
    await listen("combat-stats", (event) => renderStats(event.payload));
    await listen("capture-status", (event) => {
      if (event.payload !== view.status) renderStatus(event.payload);
    });
    await listen("capture-diagnostics", (event) => renderDiagnostics(event.payload));
    await listen("clickthrough-toggled", (event) => renderPassthrough(Boolean(event.payload)));
    try {
      renderStats(await invoke("get_combat_stats"));
    } catch (error) {
      console.error("Could not load session stats:", error);
    }
  } else {
    // Plain browser: no capture. F8 and the stats clock are simulated.
    window.addEventListener("keydown", (event) => {
      if (event.key === "F8") setPassthrough(!view.passthrough);
    });
    renderStatus("unavailable");
    $("status-text").textContent = "Preview · no capture";
    $("capture-details").textContent = "Browser preview. Use the ··· menu to add test hits.";
    mock.reset();
    setInterval(() => mock.tick(), 250);
  }
}

window.addEventListener("DOMContentLoaded", start);
