pub mod capture;
pub mod state;
pub mod vision;

use capture::{CaptureOutcome, ScreenCapture};
use state::{AppState, CombatStats, DamageEvent, ElementType, SharedState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State, Window};
use vision::VisionEngine;


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
    state.lock().set_capture_active(enabled);
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
        Some("physical") => ElementType::Physical,
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

/// Flip click-through from the global hotkey and tell the panel.
#[cfg(windows)]
fn toggle_click_through_from_hotkey(app_handle: &AppHandle, state: &SharedState) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let mut s = state.lock();
        let new_state = !s.click_through;
        if window.set_ignore_cursor_events(new_state).is_ok() {
            s.click_through = new_state;
            let _ = app_handle.emit("clickthrough-toggled", new_state);
        }
    }
}

/// F8 as a system-wide hotkey on its own thread.
///
/// The system delivers a registered hotkey whichever window has focus, and
/// independently of the capture loop. The old approach, sampling the key state
/// once per captured frame, missed presses whenever a frame took long and read
/// nothing at all while a game running as administrator had focus.
#[cfg(windows)]
fn spawn_hotkey_thread(app_handle: AppHandle, state: SharedState) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, RegisterHotKey, MOD_NOREPEAT, VK_F8,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

    thread::spawn(move || {
        const HOTKEY_ID: i32 = 1;
        let registered =
            unsafe { RegisterHotKey(None, HOTKEY_ID, MOD_NOREPEAT, u32::from(VK_F8.0)) };
        if registered.is_ok() {
            // Hotkeys registered without a window arrive on this thread's queue.
            let mut message = MSG::default();
            while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
                if message.message == WM_HOTKEY && message.wParam.0 == HOTKEY_ID as usize {
                    toggle_click_through_from_hotkey(&app_handle, &state);
                }
            }
            return;
        }

        // Another program owns F8. Fall back to watching the key directly.
        eprintln!("[Hotkey] F8 is registered by another program; polling the key instead.");
        let mut was_down = false;
        loop {
            let down = unsafe { (GetAsyncKeyState(i32::from(VK_F8.0)) as u16 & 0x8000) != 0 };
            if down && !was_down {
                toggle_click_through_from_hotkey(&app_handle, &state);
            }
            was_down = down;
            thread::sleep(Duration::from_millis(15));
        }
    });
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

            // Set initial click-through and exclude overlay from capture (prevents self-capture feedback)
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_ignore_cursor_events(false); // Start interactive or toggleable
                let _ = window.set_always_on_top(true);
                #[cfg(windows)]
                if let Ok(hwnd) = window.hwnd() {
                    use windows::Win32::Foundation::HWND;
                    use windows::Win32::UI::WindowsAndMessaging::{
                        SetWindowDisplayAffinity, WINDOW_DISPLAY_AFFINITY,
                    };
                    unsafe {
                        let _ = SetWindowDisplayAffinity(
                            HWND(hwnd.0 as _),
                            WINDOW_DISPLAY_AFFINITY(17),
                        );
                    }
                }
            }

            #[cfg(windows)]
            spawn_hotkey_thread(app.handle().clone(), shared_state.clone());

            // Spawn background vision & capture thread
            thread::spawn(move || {
                let mut capture = ScreenCapture::new();
                let mut vision = match VisionEngine::try_new() {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!(
                            "[VisionEngine ERROR] Failed to initialize VisionEngine: {}",
                            e
                        );
                        return;
                    }
                };
                let mut last_stats_emit = Instant::now();
                let mut vision_generation = state_clone.lock().vision_generation;
                let mut last_capture_status = "";
                let mut diagnostics_started = Instant::now();
                let mut diagnostic_frames = 0u32;
                let mut diagnostic_candidates = 0usize;
                let mut diagnostic_hits = 0usize;
                let mut diagnostic_ocr_ms = 0.0;
                let mut diagnostic_size = (0, 0);

                while running_clone.load(Ordering::Relaxed) {
                    let loop_start = Instant::now();

                    let (is_active, generation) = {
                        let s = state_clone.lock();
                        (s.capture_active, s.vision_generation)
                    };
                    if generation != vision_generation {
                        vision.reset();
                        vision_generation = generation;
                    }
                    if is_active {
                        let hits = match capture.capture() {
                            CaptureOutcome::Frame(frame) => {
                                if last_capture_status != "tracking" {
                                    let _ = app_handle.emit("capture-status", "tracking");
                                    last_capture_status = "tracking";
                                }
                                let observed_at = Instant::now();
                                let analysis = vision.analyze_frame_at(&frame, observed_at);
                                diagnostic_frames += 1;
                                diagnostic_candidates += analysis.detections.len();
                                diagnostic_ocr_ms += observed_at.elapsed().as_secs_f64() * 1000.0;
                                diagnostic_size = (frame.width, frame.height);
                                analysis.hits
                            }
                            CaptureOutcome::NoNewFrame => vision.no_new_frame(Instant::now()),
                            CaptureOutcome::Unavailable => {
                                vision.reset();
                                if last_capture_status != "unavailable" {
                                    let _ = app_handle.emit("capture-status", "unavailable");
                                    last_capture_status = "unavailable";
                                }
                                Vec::new()
                            }
                        };
                        if !hits.is_empty() {
                            diagnostic_hits += hits.len();
                            let mut s = state_clone.lock();
                            // Reset/pause may have happened while OCR was running.
                            if s.accepts_vision_generation(generation) {
                                for hit in hits {
                                    let event = s.record_hit(
                                        hit.value,
                                        hit.element,
                                        hit.is_crit,
                                        hit.x,
                                        hit.y,
                                    );
                                    let _ = app_handle.emit("damage-hit", &event);
                                }
                            }
                        }
                    } else {
                        vision.reset();
                        if last_capture_status != "paused" {
                            let _ = app_handle.emit("capture-status", "paused");
                            last_capture_status = "paused";
                        }
                    }

                    // Emit rolling stats update 4 times a second
                    if diagnostics_started.elapsed() >= Duration::from_secs(2) {
                        let fps = f64::from(diagnostic_frames)
                            / diagnostics_started.elapsed().as_secs_f64();
                        let average_ms = if diagnostic_frames > 0 {
                            diagnostic_ocr_ms / f64::from(diagnostic_frames)
                        } else {
                            0.0
                        };
                        let diagnostics = serde_json::json!({
                            "status": last_capture_status, "fps": fps,
                            "width": diagnostic_size.0, "height": diagnostic_size.1,
                            "candidates": diagnostic_candidates, "hits": diagnostic_hits,
                            "processing_ms": average_ms,
                        });
                        if last_capture_status == "tracking" {
                            println!("[Capture diagnostics] {diagnostics}");
                        }
                        let _ = app_handle.emit("capture-diagnostics", &diagnostics);
                        diagnostics_started = Instant::now();
                        diagnostic_frames = 0;
                        diagnostic_candidates = 0;
                        diagnostic_hits = 0;
                        diagnostic_ocr_ms = 0.0;
                    }

                    if last_stats_emit.elapsed() >= Duration::from_millis(250) {
                        let stats = state_clone.lock().compute_stats();
                        let _ = app_handle.emit("combat-stats", &stats);
                        // Repeat state so a frontend that subscribes after startup recovers.
                        let _ = app_handle.emit("capture-status", last_capture_status);
                        last_stats_emit = Instant::now();
                    }

                    // While tracking, the capture call itself waits for the next
                    // desktop frame. Sleeping on top of that would round up to the
                    // system timer tick and halve the frame rate. Only idle states
                    // need pacing.
                    if last_capture_status != "tracking" {
                        let elapsed = loop_start.elapsed();
                        if elapsed < Duration::from_millis(16) {
                            thread::sleep(Duration::from_millis(16) - elapsed);
                        }
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
