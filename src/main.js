// GenshinDamageLens - Frontend Event Controller & Visualizer

// Tauri API Access
const tauri = window.__TAURI__ || null;
const invoke = tauri ? tauri.core.invoke : null;
const listen = tauri ? tauri.event.listen : null;

// DOM Elements
const dpsDisplay = document.getElementById("dps-display");
const dpsFill = document.getElementById("dps-fill");
const peakCard = document.getElementById("peak-card");
const peakDisplay = document.getElementById("peak-display");
const peakElementBadge = document.getElementById("peak-element-badge");
const peakMetaText = document.getElementById("peak-meta-text");
const critRateDisplay = document.getElementById("crit-rate-display");
const totalDamageDisplay = document.getElementById("total-damage-display");
const totalHitsDisplay = document.getElementById("total-hits-display");
const elementalBar = document.getElementById("elemental-bar");
const hitTickerList = document.getElementById("hit-ticker-list");
const feedCount = document.getElementById("feed-count");
const damageLayer = document.getElementById("damage-layer");
const hudContainer = document.getElementById("hud-container");

const btnClickthrough = document.getElementById("btn-clickthrough");
const modeIcon = document.getElementById("mode-icon");
const modeLabel = document.getElementById("mode-label");
const btnSimulate = document.getElementById("btn-simulate");
const btnAutoSim = document.getElementById("btn-auto-sim");
const btnReset = document.getElementById("btn-reset");
const btnCollapse = document.getElementById("btn-collapse");
const collapseIcon = document.getElementById("collapse-icon");

// State
let isPassthrough = false;
let isAutoSimulating = false;
let autoSimInterval = null;
let isCollapsed = false;

// Format numbers nicely (e.g. 1,420,500 or 1.42M)
function formatNumber(num) {
  return num.toLocaleString();
}

function formatCompact(num) {
  if (num >= 1_000_000_000) return (num / 1_000_000_000).toFixed(2) + "B";
  if (num >= 1_000_000) return (num / 1_000_000).toFixed(2) + "M";
  if (num >= 1_000) return (num / 1_000).toFixed(1) + "K";
  return num.toString();
}

// Spawn floating in-game damage number
function spawnFloatingHit(event) {
  const hitEl = document.createElement("div");
  hitEl.className = `floating-hit ${event.element} ${event.is_crit ? "crit" : "normal"}`;
  hitEl.textContent = `${event.value.toLocaleString()}${event.is_crit ? "!" : ""}`;

  // Use screen coordinates or fallback to random screen center
  const screenW = window.innerWidth;
  const screenH = window.innerHeight;

  let x = event.x > 0 ? event.x : screenW * 0.45 + (Math.random() * 200 - 100);
  let y = event.y > 0 ? event.y : screenH * 0.45 + (Math.random() * 150 - 75);

  // Clamp within bounds
  x = Math.max(80, Math.min(screenW - 80, x));
  y = Math.max(120, Math.min(screenH - 120, y));

  hitEl.style.left = `${x}px`;
  hitEl.style.top = `${y}px`;

  damageLayer.appendChild(hitEl);

  setTimeout(() => {
    hitEl.remove();
  }, 1300);
}

// Append hit to recent ticker
function addHitToTicker(event) {
  // Remove empty placeholder
  const hint = hitTickerList.querySelector(".empty-feed-hint");
  if (hint) hint.remove();

  const item = document.createElement("div");
  item.className = `feed-item ${event.element}`;
  item.innerHTML = `
    <div class="feed-left">
      <span class="feed-badge">${event.element}</span>
      ${event.is_crit ? '<span class="feed-crit-tag">CRIT</span>' : ""}
    </div>
    <span class="feed-value">${event.value.toLocaleString()}</span>
  `;

  hitTickerList.prepend(item);

  // Keep max 6 items
  while (hitTickerList.children.length > 6) {
    hitTickerList.lastElementChild.remove();
  }

  feedCount.textContent = `${hitTickerList.children.length} recent`;
}

// Update UI from CombatStats
function updateCombatStats(stats) {
  // 1. DPS
  dpsDisplay.textContent = formatNumber(stats.dps);
  const fillPct = Math.min(100, (stats.dps / 150_000) * 100);
  dpsFill.style.width = `${fillPct}%`;

  // 2. Peak Hit
  if (stats.peak_hit > 0) {
    peakDisplay.textContent = formatNumber(stats.peak_hit);
    peakElementBadge.textContent = stats.peak_element.toUpperCase();
    peakMetaText.textContent = `Elemental Peak Damage`;

    // Remove old element classes and set new one
    peakCard.className = `hud-card peak-card ${stats.peak_element}`;
  } else {
    peakDisplay.textContent = "0";
    peakElementBadge.textContent = "PHYSICAL";
    peakCard.className = "hud-card peak-card";
    peakMetaText.textContent = "No combat data yet";
  }

  // 3. Stats Grid
  critRateDisplay.textContent = `CRIT ${stats.crit_rate_pct.toFixed(1)}%`;
  totalDamageDisplay.textContent = formatCompact(stats.total_damage);
  totalHitsDisplay.textContent = stats.total_hits.toLocaleString();

  // 4. Elemental Breakdown Bar
  renderElementalBar(stats.elemental_breakdown, stats.total_damage);
}

// Render stacked elemental distribution
function renderElementalBar(breakdown, total) {
  elementalBar.innerHTML = "";
  if (!breakdown || total === 0) {
    elementalBar.innerHTML = '<div class="elem-segment physical" style="width: 100%"></div>';
    return;
  }

  for (const [element, amount] of Object.entries(breakdown)) {
    const pct = ((amount / total) * 100).toFixed(1);
    if (pct > 0) {
      const seg = document.createElement("div");
      seg.className = `elem-segment ${element.toLowerCase()}`;
      seg.style.width = `${pct}%`;
      seg.title = `${element.toUpperCase()}: ${pct}% (${formatCompact(amount)})`;
      elementalBar.appendChild(seg);
    }
  }
}

// Setup Event Listeners
async function setupEventListeners() {
  if (listen) {
    // Listen to real-time damage hit
    await listen("damage-hit", (event) => {
      spawnFloatingHit(event.payload);
      addHitToTicker(event.payload);
    });

    // Listen to rolling combat stats
    await listen("combat-stats", (event) => {
      updateCombatStats(event.payload);
    });

    // Listen to global clickthrough toggle (from backend F8 hotkey when unfocused)
    await listen("clickthrough-toggled", (event) => {
      isPassthrough = Boolean(event.payload);
      applyClickthroughUI(isPassthrough);
    });
  }

  // Toggle Click-Through mode
  btnClickthrough.addEventListener("click", toggleClickthrough);

  // Fallback local keydown listener when overlay happens to have focus
  window.addEventListener("keydown", (e) => {
    if (e.key === "F8") {
      toggleClickthrough();
    }
  });

  // Trigger test hit
  btnSimulate.addEventListener("click", async () => {
    if (invoke) {
      await invoke("simulate_hit", { element: null, isCrit: null });
    } else {
      mockSimulationHit();
    }
  });

  // Toggle auto simulation loop
  btnAutoSim.addEventListener("click", () => {
    isAutoSimulating = !isAutoSimulating;
    if (isAutoSimulating) {
      btnAutoSim.style.background = "rgba(16, 185, 129, 0.3)";
      btnAutoSim.style.borderColor = "#10b981";
      autoSimInterval = setInterval(async () => {
        if (invoke) {
          await invoke("simulate_hit", { element: null, isCrit: null });
        } else {
          mockSimulationHit();
        }
      }, 750);
    } else {
      btnAutoSim.style.background = "";
      btnAutoSim.style.borderColor = "";
      clearInterval(autoSimInterval);
      autoSimInterval = null;
    }
  });

  // Reset stats
  btnReset.addEventListener("click", async () => {
    if (invoke) {
      const stats = await invoke("reset_stats");
      updateCombatStats(stats);
      hitTickerList.innerHTML = '<div class="empty-feed-hint">Waiting for combat damage...</div>';
      feedCount.textContent = "0 hits";
    } else {
      mockReset();
    }
  });

  // Collapse / Minimize
  btnCollapse.addEventListener("click", () => {
    isCollapsed = !isCollapsed;
    hudContainer.classList.toggle("collapsed", isCollapsed);
    collapseIcon.textContent = isCollapsed ? "➕" : "➖";
  });
}

function applyClickthroughUI(passthrough) {
  if (passthrough) {
    btnClickthrough.classList.add("passthrough");
    modeIcon.textContent = "🛡️";
    modeLabel.textContent = "Passthrough (F8)";
  } else {
    btnClickthrough.classList.remove("passthrough");
    modeIcon.textContent = "🖱️";
    modeLabel.textContent = "Interactive";
  }
}

// Toggle Passthrough
async function toggleClickthrough() {
  isPassthrough = !isPassthrough;
  applyClickthroughUI(isPassthrough);

  if (invoke) {
    try {
      await invoke("toggle_click_through", { ignore: isPassthrough });
    } catch (err) {
      console.warn("Could not set click-through:", err);
    }
  }
}

// Browser Mock Simulation (for standalone testing outside Tauri)
let mockStats = {
  dps: 0,
  peak_hit: 0,
  peak_element: "pyro",
  total_damage: 0,
  total_hits: 0,
  crit_hits: 0,
  crit_rate_pct: 0,
  elemental_breakdown: {},
};

function mockSimulationHit() {
  const elements = ["pyro", "hydro", "cryo", "electro", "dendro", "anemo", "geo", "physical"];
  const elem = elements[Math.floor(Math.random() * elements.length)];
  const isCrit = Math.random() < 0.65;
  const value = isCrit
    ? Math.floor(35000 + Math.random() * 120000)
    : Math.floor(5000 + Math.random() * 20000);

  const event = {
    id: Date.now(),
    timestamp_ms: Date.now(),
    value,
    element: elem,
    is_crit: isCrit,
    x: 0,
    y: 0,
  };

  mockStats.total_damage += value;
  mockStats.total_hits += 1;
  if (isCrit) mockStats.crit_hits += 1;
  mockStats.crit_rate_pct = (mockStats.crit_hits / mockStats.total_hits) * 100;
  mockStats.dps = Math.floor(mockStats.dps * 0.7 + value * 0.3);

  if (value > mockStats.peak_hit) {
    mockStats.peak_hit = value;
    mockStats.peak_element = elem;
  }

  mockStats.elemental_breakdown[elem] = (mockStats.elemental_breakdown[elem] || 0) + value;

  spawnFloatingHit(event);
  addHitToTicker(event);
  updateCombatStats(mockStats);
}

function mockReset() {
  mockStats = {
    dps: 0,
    peak_hit: 0,
    peak_element: "physical",
    total_damage: 0,
    total_hits: 0,
    crit_hits: 0,
    crit_rate_pct: 0,
    elemental_breakdown: {},
  };
  updateCombatStats(mockStats);
  hitTickerList.innerHTML = '<div class="empty-feed-hint">Waiting for combat damage...</div>';
  feedCount.textContent = "0 hits";
}

// Initialize on Load
window.addEventListener("DOMContentLoaded", async () => {
  setupEventListeners();

  // Initial fetch from backend
  if (invoke) {
    try {
      const stats = await invoke("get_combat_stats");
      updateCombatStats(stats);
    } catch (e) {
      console.error("Failed to load initial combat stats:", e);
    }
  }
});
