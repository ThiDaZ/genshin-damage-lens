pub mod capture;
pub mod state;
pub mod vision;

use capture::ScreenCapture;
use state::{AppState, CombatStats, DamageEvent, ElementType, SharedState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State, Window};
use vision::VisionEngine;

#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F8};

#[tauri::command]
fn get_combat_stats(state: State<'_, SharedState>) -> CombatStats {
    state.lock().compute_stats()
}

#[tauri::command]
fn reset_stats(state: State<'_, SharedState>, app_handle: AppHandle) -> CombatStats {
    let mut s = state.lock();
    s.reset();
    let stats = s.compute_stats();
    let _ = app_handle.emit("combat-stats", &stats);
    stats
}

#[tauri::command]
fn toggle_click_through(
    window: Window,
    ignore: bool,
    state: State<'_, SharedState>,
    app_handle: AppHandle,
) -> Result<bool, String> {
    window
        .set_ignore_cursor_events(ignore)
        .map_err(|e| e.to_string())?;

    state.lock().click_through = ignore;
    let _ = app_handle.emit("clickthrough-toggled", ignore);
    Ok(ignore)
}

#[tauri::command]
fn set_capture_active(enabled: bool, state: State<'_, SharedState>) -> bool {
    state.lock().capture_active = enabled;
    enabled
}

#[tauri::command]
fn simulate_hit(
    element: Option<String>,
    is_crit: Option<bool>,
    state: State<'_, SharedState>,
    app_handle: AppHandle,
) -> DamageEvent {
    let elem = match element.as_deref() {
        Some("pyro") => ElementType::Pyro,
        Some("hydro") => ElementType::Hydro,
        Some("cryo") => ElementType::Cryo,
        Some("electro") => ElementType::Electro,
        Some("dendro") => ElementType::Dendro,
        Some("anemo") => ElementType::Anemo,
        Some("geo") => ElementType::Geo,
        _ => {
            // Random element
            let all = [
                ElementType::Pyro,
                ElementType::Hydro,
                ElementType::Cryo,
                ElementType::Electro,
                ElementType::Dendro,
                ElementType::Anemo,
                ElementType::Geo,
                ElementType::Physical,
            ];
            let idx = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos() as usize)
                % all.len();
            all[idx]
        }
    };

    let crit = is_crit.unwrap_or_else(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        (nanos % 100) < 65 // ~65% crit rate
    });

    // Realistic Genshin numbers: normal 5k-25k, crit 35k-180k
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_micros();
    let base = if crit {
        35_000 + (nanos % 145_000)
    } else {
        4_000 + (nanos % 21_000)
    };

    let x = 600 + ((nanos % 700) as i32);
    let y = 350 + (((nanos / 10) % 400) as i32);

    let event = state.lock().record_hit(base, elem, crit, x, y);
    let _ = app_handle.emit("damage-hit", &event);

    let stats = state.lock().compute_stats();
    let _ = app_handle.emit("combat-stats", &stats);

    event
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let shared_state: SharedState = Arc::new(parking_lot::Mutex::new(AppState::new()));
    let running = Arc::new(AtomicBool::new(true));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(shared_state.clone())
        .invoke_handler(tauri::generate_handler![
            get_combat_stats,
            reset_stats,
            toggle_click_through,
            set_capture_active,
            simulate_hit,
        ])
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let state_clone = shared_state.clone();
            let running_clone = running.clone();

            // Set initial click-through on main overlay window
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_ignore_cursor_events(false); // Start interactive or toggleable
                let _ = window.set_always_on_top(true);
            }

            // Spawn background vision & capture thread
            thread::spawn(move || {
                let mut capture = ScreenCapture::new();
                let mut vision = VisionEngine::new();
                let mut last_stats_emit = Instant::now();
                #[cfg(windows)]
                let mut f8_was_down = false;
                #[cfg(windows)]
                let mut last_f8_time = Instant::now();

                while running_clone.load(Ordering::Relaxed) {
                    let loop_start = Instant::now();

                    // Global F8 Hotkey check (works even when overlay or game is unfocused)
                    #[cfg(windows)]
                    {
                        let f8_down = unsafe { (GetAsyncKeyState(VK_F8.0 as i32) as u16 & 0x8000) != 0 };
                        if f8_down && !f8_was_down && last_f8_time.elapsed() >= Duration::from_millis(250) {
                            last_f8_time = Instant::now();
                            if let Some(window) = app_handle.get_webview_window("main") {
                                let mut s = state_clone.lock();
                                let new_state = !s.click_through;
                                s.click_through = new_state;
                                let _ = window.set_ignore_cursor_events(new_state);
                                let _ = app_handle.emit("clickthrough-toggled", new_state);
                            }
                        }
                        f8_was_down = f8_down;
                    }

                    let is_active = state_clone.lock().capture_active;
                    if is_active {
                        if let Some(frame) = capture.capture() {
                            let hits = vision.process_frame(&frame);
                            if !hits.is_empty() {
                                let mut s = state_clone.lock();
                                for hit in hits {
                                    let event = s.record_hit(hit.value, hit.element, hit.is_crit, hit.x, hit.y);
                                    let _ = app_handle.emit("damage-hit", &event);
                                }
                            }
                        } else {
                            // Game is minimized or paused - clear tracking history
                            vision.reset();
                        }
                    }

                    // Emit rolling stats update 4 times a second
                    if last_stats_emit.elapsed() >= Duration::from_millis(250) {
                        let stats = state_clone.lock().compute_stats();
                        let _ = app_handle.emit("combat-stats", &stats);
                        last_stats_emit = Instant::now();
                    }

                    // Maintain ~60 FPS or sleep when idle
                    let elapsed = loop_start.elapsed();
                    if elapsed < Duration::from_millis(16) {
                        thread::sleep(Duration::from_millis(16) - elapsed);
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
