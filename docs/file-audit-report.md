# File audit report

Audited: 2026-11-13, branch `dev`, HEAD `7ff7105`.
Scope: every tracked file plus the one untracked file in the working tree. Gitignored local material (`sample_video/`, `scratch/`, `node_modules/`, `target/`) is noted only where code or docs depend on it.

This is a static audit. Checks run: `node --check src/main.js` (passes), reference searches across the source, and a review of the working-tree diff. `cargo test` and `cargo check` were not run in this audit.

## 1. Working-tree status

| State | File | Notes |
|---|---|---|
| Modified | `README.md` | +10 lines (recognition/diagnostics wording) |
| Modified | `docs/recognition.md` | +37 lines (replay contract, live diagnosis) |
| Modified | `src-tauri/examples/capture_probe.rs` | +89/−~25, sampling probe rework |
| Modified | `src-tauri/src/lib.rs` | +33 lines, vision generation / state handling |
| Modified | `src-tauri/src/vision/filter.rs` | +29/−~6, glyph outline and luminance changes |
| Modified | `src-tauri/src/vision/matcher.rs` | +77 lines, fragment recovery and tall-glyph handling |
| Modified | `src-tauri/src/vision/mod.rs` | +13/−~1 |
| Modified | `src-tauri/tests/fixtures/README.md` | +3 lines, provenance for `physical-impact.png` |
| Modified | `src-tauri/tests/recognition.rs` | +11 lines, new Physical 1215 regression |
| Modified | `src/index.html` | +1 line |
| Modified | `src/main.js` | +17/−~1, diagnostics display |
| Untracked | `src-tauri/tests/fixtures/physical-impact.png` | **Not committed. See finding F1.** |

Total: 11 modified files (294 insertions, 26 deletions) and 1 untracked file. The working tree is not clean; the recognition work is not yet committed.

## 2. Tracked file inventory

| Area | Files | Role | Status |
|---|---|---|---|
| Frontend | `src/index.html`, `src/main.js`, `src/styles.css` | Overlay UI, IPC, controls | `main.js` syntax OK |
| Frontend (unused) | `src/assets/javascript.svg`, `src/assets/tauri.svg` | Scaffold template images | Not referenced anywhere. See F4 |
| Tauri shell | `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`, `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`, `src-tauri/build.rs` | Entry point, commands, window and security config | Reviewed. See F5 |
| Capture | `src-tauri/src/capture/{mod,dxgi,window_finder}.rs` | DXGI desktop capture, window and display selection, crop | Reviewed; no findings |
| Vision | `src-tauri/src/vision/{mod,filter,matcher,tracker,replay}.rs` | Segmentation, template OCR, tracking, replay scoring | Reviewed; largest files (matcher 1114 lines, filter 885, mod 879) |
| State | `src-tauri/src/state.rs` | Session hits, DPS aggregation | Reviewed; no findings |
| Examples | `src-tauri/examples/{capture_probe,inspect_frame,replay}.rs` | Diagnostics and replay CLIs | `replay` is the documented validation command |
| Tests | `src-tauri/tests/recognition.rs` | Full-frame regressions | Depends on F1 |
| Fixtures | `src-tauri/tests/fixtures/*` (8 PNGs, 2 JSON, README) | Labeled regression images and manifests | ~11 MB of PNG. Provenance documented |
| Models | `src-tauri/assets/text-detection.rten` (2.5 MB), `text-recognition.rten` (9.7 MB) | ONNX-style OCR models | Not referenced by any code or config. See F3 |
| Docs | `README.md`, `docs/recognition.md`, `src-tauri/tests/fixtures/README.md` | Usage, evaluation contract, provenance | Consistent with code paths checked |
| Build/config | `package.json`, `pnpm-lock.yaml`, `run.cmd`, `src-tauri/Cargo.toml`, `Cargo.lock`, `.gitignore`, `src-tauri/.gitignore` | Dependencies, dev and build scripts | `run.cmd` kills any running app before `tauri` |
| Icons | `src-tauri/icons/*` | App icons for Windows, macOS and Store | Standard generated set |

## 3. Findings

Ranked by impact.

**F1 (high). The regression test depends on an uncommitted file.**
`src-tauri/tests/recognition.rs:60` loads `physical-impact.png`, and `tests/fixtures/README.md` documents it. The file is untracked. A fresh clone will fail this test with `tracked recognition fixture must exist`.
*Fix:* `git add src-tauri/tests/fixtures/physical-impact.png` together with the other pending changes.

**F2 (medium). Vision tests reference gitignored local folders.**
`src-tauri/src/vision/mod.rs` reads `../sample_video/…`, `../scratch/combat_frames`, `../scratch/elements/…` and `../sample_video/roam_frames`. Those paths are gitignored. Several tests print a skip message when absent, so they pass silently on a clean checkout. The README says this, but a passing `cargo test` can hide that those tests did not run.
*Fix:* report skipped tests explicitly, or move them behind an opt-in `#[ignore]` or environment variable.

**F3 (low–medium). About 12 MB of OCR models are tracked but unused.**
`src-tauri/assets/text-detection.rten` and `text-recognition.rten` are not referenced by any Rust, config, or doc file. `docs/recognition.md` and `README.md` describe neural OCR as an evaluation candidate, not an active backend. The models are therefore dead weight in the repo and in the bundle if Tauri packages `assets/`.
*Fix:* keep them only if the evaluation will use them soon. Otherwise move them out of the tracked tree, or record their intended use in `docs/recognition.md`.

**F4 (low). Scaffold leftovers.**
`src/assets/javascript.svg` and `src/assets/tauri.svg` are not referenced by `index.html`, `main.js`, or `styles.css`.
*Fix:* delete both.

**F5 (medium, security posture). Content Security Policy is disabled.**
`src-tauri/tauri.conf.json` sets `"csp": null`. The overlay renders only local content, but any injected markup would run with full Tauri IPC access. The capability file is minimal (`core:default`, `opener:default`), which limits the impact.
*Fix:* set a restrictive CSP such as `default-src 'self'; style-src 'self'; img-src 'self' data:`, then check that the overlay still runs. Consider whether `opener:default` is needed.

**F6 (low). Line-ending churn.**
Git warns that LF will become CRLF on the next touch for every modified text file. This means `core.autocrlf` or the working-copy line endings are inconsistent with the index. It will produce noisy diffs.
*Fix:* add a `.gitattributes` (for example `* text=auto eol=lf`) and normalize once in a separate commit.

**F7 (info). The `.gitignore` contains an unrelated entry.**
`gemini.md` is listed in `.gitignore`. It is harmless, but it is unexplained. Also, `src-tauri/scratch_ocrs/target/` is listed although no such folder exists in the repo.
*Fix:* remove stale entries when the ignore file is next edited.

**F8 (info). Large fixture images.**
The fixture PNGs total about 11 MB. This is acceptable for regression data, but each new screenshot adds permanent repo weight. Consider Git LFS if the fixture set grows.

## 4. Things checked and found consistent

- Module declarations match the files on disk (`capture`, `state`, `vision::{filter,matcher,mod,replay,tracker}`).
- The replay commands in `README.md` point to existing files: `src-tauri/tests/fixtures/recognition.json` and `electro-events.json`.
- Fixture labels in `recognition.json` and `electro-events.json` match the fixture README provenance.
- `tauri_plugin_opener` is initialized in `lib.rs` and granted in the capability file.
- `src/main.js` passes `node --check`.
- No `TODO`, `FIXME`, `todo!` or `unimplemented!` markers were found in `src-tauri/src` or `src/main.js`.

## 5. Recommended order

1. Commit `physical-impact.png` with the pending recognition changes (F1).
2. Make skipped local-folder tests visible (F2).
3. Decide on the OCR models (F3) and delete the scaffold SVGs (F4).
4. Tighten the CSP (F5).
5. Normalize line endings in a separate commit (F6).

## 6. Resolution

Status after the fix branch. Commit hashes are on the branch named in the commit log.

| Finding | Status | Change |
|---|---|---|
| F1 | Resolved | `physical-impact.png` and the recognition work were committed to `dev` by `1e890da`/`443191d` before this branch. The file is now tracked. |
| F2 | Resolved | Local-data tests in `src-tauri/src/vision/mod.rs` are marked `#[ignore]`. The default `cargo test` run no longer reports them as passing. Run them with `-- --ignored`. README updated. |
| F3 | Open (decision) | The OCR models are still tracked and unreferenced. Removing them is a product decision, so they were kept. |
| F4 | Resolved | `src/assets/javascript.svg` and `src/assets/tauri.svg` removed. |
| F5 | Resolved, runtime not verified | `csp` set in `tauri.conf.json`. The overlay's inline `style` attribute is allowed by `'unsafe-inline'`. The app was not launched to confirm this. |
| F6 | Resolved | `.gitattributes` added with `* text=auto`. |
| F7 | Partly resolved | Removed the nonexistent `src-tauri/scratch_ocrs/target/` entry. `gemini.md` kept because its purpose is not recorded. |
| F8 | No action | Informational. |
