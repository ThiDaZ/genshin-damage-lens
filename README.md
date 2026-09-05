# ⚔️ GenshinDamageLens

> **Ultra-lightweight, high-performance external desktop damage tracker and overlay for Genshin Impact built with Tauri v2, Rust, Direct3D 11 / DXGI, and Vanilla HTML5/CSS3.**

---

## 🛡️ Anti-Cheat & Operational Safety

- **Strictly External:** GenshinDamageLens operates **completely out-of-process**. It does **NOT** inject DLLs into the game, inspect game process memory (`ReadProcessMemory`), or hook graphics swapchains.
- **Zero-Copy DXGI Capture:** All frame data is acquired purely via the Windows Desktop Duplication API (DXGI / Direct3D 11) at the OS level.
- **Ultra-Low Overhead:** Targets `< 60 MB` RAM footprint and `< 2%` CPU usage during active combat tracking.
- **Input Passthrough:** The overlay window renders with a transparent, frameless canvas and passes all mouse and keyboard inputs directly through to the game (`set_ignore_cursor_events(true)`).

---

## 🚀 Key Features

- **⚡ Real-Time Vision & OCR Pipeline:**
  - Fast integer RGB/HSV color segmentation isolating high-contrast elemental damage numbers.
  - Multi-element recognition: **Pyro**, **Hydro**, **Cryo**, **Electro**, **Dendro**, **Anemo**, **Geo**, and **Physical**.
  - Normalized Cross-Correlation (NCC) template matching customized for Genshin's distinctive damage font glyphs.
  - Critical hit detection (burst markers, larger font scales, and exclamation marks).

- **🎯 Centroid Trajectory Tracker & Deduplicator:**
  - Follows floating damage numbers across successive frames as they spawn and float upward.
  - Consolidates each hit event to emit only the confirmed **peak numerical value**, completely eliminating duplicate counts.

- **💎 Sleek Glassmorphic Overlay HUD:**
  - **Live Rolling DPS Meter:** 5-second sliding window with glowing intensity indicator.
  - **Peak Hit Showcase:** Glowing elemental trophy card highlighting your session's highest hit.
  - **Combat Feed Ticker:** Real-time stream of recent damage hits with elemental tags and crit badges.
  - **Elemental Damage Distribution:** Dynamic stacked percentage bar of elemental damage types.
  - **Interactive vs. Passthrough Mode:** Seamlessly toggle click-through overlay mode with `F8` or the toolbar button to drag/reposition panels.
  - **Built-in Demo Simulation Loop:** Integrated test simulator to test and showcase animations even without running the game.

---

## 🏗️ Project Architecture

```text
GenshinDamageLens/
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json         # Transparent, frameless, topmost window configuration
│   └── src/
│       ├── main.rs             # Tauri binary entrypoint
│       ├── lib.rs              # Tauri setup, IPC commands, and background capture loop
│       ├── capture/
│       │   ├── mod.rs          # Screen capture orchestrator & RawFrame
│       │   ├── dxgi.rs         # Direct3D 11 DXGI Desktop Duplication frame grabber
│       │   └── window_finder.rs# Genshin window detector & viewport cropper
│       ├── vision/
│       │   ├── mod.rs          # Vision pipeline coordinator
│       │   ├── filter.rs       # RGB/HSV elemental classifier & connected components
│       │   ├── matcher.rs      # Digit OCR & normalized cross-correlation matcher
│       │   └── tracker.rs      # Centroid tracker & peak hit deduplicator
│       └── state.rs            # Thread-safe combat session state & DPS calculator
├── src/                        # Modern Web Overlay HUD
│   ├── index.html              # Glassmorphic HUD DOM structure
│   ├── styles.css              # Custom neon glows, elemental themes & animations
│   └── main.js                 # Tauri IPC event listener & dynamic UI renderer
├── package.json
└── README.md
```

---

## 🛠️ Getting Started

### Prerequisites
- Windows 10/11
- [Node.js](https://nodejs.org/) & [pnpm](https://pnpm.io/)
- [Rust & Cargo](https://rustup.rs/) (MSVC toolchain: `x86_64-pc-windows-msvc`)

### Installation & Running Locally

1. **Install Frontend Dependencies:**
   ```bash
   pnpm install
   ```

2. **Run Development Server with Tauri Overlay:**
   ```bash
   pnpm run dev
   ```

3. **Build Production Binary:**
   ```bash
   pnpm run build
   ```

---

## 🧪 Testing & Verification

Run the automated Rust test suite covering elemental classification, digit template matching, centroid tracking/deduplication, and combat stats:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

All 4 unit test suites will execute and validate the core vision, OCR, and tracking algorithms.

---

## 🎮 Overlay Controls

| Control | Action |
|---|---|
| `F8` or **Mode Button** | Toggle between **Interactive** (click buttons/drag) and **Passthrough** (click passes directly to game) |
| **⚡ Test Hit** | Trigger an instantaneous simulated damage hit |
| **🔄 Demo Loop** | Toggle an automated combat simulation loop for UI demonstration |
| **🗑️ Reset** | Clear session combat stats, DPS meter, and peak hit records |
| **➖ Collapse** | Minimize the floating HUD card into a compact toolbar |
