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

/// Hide the overlay from screen capture, or let recorders see it.
#[cfg(windows)]
fn apply_capture_visibility(window: &tauri::WebviewWindow, visible: bool) -> Result<(), String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
    };
    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    let affinity = if visible {
        WDA_NONE
    } else {
        WDA_EXCLUDEFROMCAPTURE
    };
    unsafe { SetWindowDisplayAffinity(HWND(hwnd.0 as _), affinity) }.map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn apply_capture_visibility(_: &tauri::WebviewWindow, _: bool) -> Result<(), String> {
    Ok(())
}

/// Recorders such as OBS or Streamlabs can only show the overlay when it is
/// not excluded from capture. Our own capture then sees it as well, so the
/// panel's box is blanked before reading damage (see `mask_panel`).
#[tauri::command]
fn set_show_in_recordings(
    visible: bool,
    window: tauri::WebviewWindow,
    state: State<'_, SharedState>,
) -> Result<bool, String> {
    apply_capture_visibility(&window, visible)?;
    state.lock().show_in_recordings = visible;
    Ok(visible)
}

/// The frontend reports where the panel sits inside the overlay window.
#[tauri::command]
fn set_panel_rect(x: f64, y: f64, width: f64, height: f64, state: State<'_, SharedState>) {
    let valid = [x, y, width, height].iter().all(|v| v.is_finite()) && width > 0.0 && height > 0.0;
    state.lock().panel_rect = valid.then(|| {
        (
            x.floor() as i32,
            y.floor() as i32,
            width.ceil() as u32,
            height.ceil() as u32,
        )
    });
}

/// Paint the overlay panel black in a captured frame so its own numbers are
/// never read as damage. `origin` is the overlay window's screen position.
fn mask_panel(frame: &mut capture::RawFrame, origin: (i32, i32), rect: (i32, i32, u32, u32)) {
    // A few pixels of margin cover the panel's shadow edge and rounding.
    const MARGIN: i64 = 6;
    let left = i64::from(origin.0) + i64::from(rect.0) - MARGIN - i64::from(frame.screen_x);
    let top = i64::from(origin.1) + i64::from(rect.1) - MARGIN - i64::from(frame.screen_y);
    let right = left + i64::from(rect.2) + 2 * MARGIN;
    let bottom = top + i64::from(rect.3) + 2 * MARGIN;
    let x0 = left.clamp(0, i64::from(frame.width)) as usize;
    let x1 = right.clamp(0, i64::from(frame.width)) as usize;
    let y0 = top.clamp(0, i64::from(frame.height)) as usize;
    let y1 = bottom.clamp(0, i64::from(frame.height)) as usize;
    if x0 >= x1 {
        return;
    }
    for y in y0..y1 {
        let row = y * frame.stride;
        if let Some(pixels) = frame.data.get_mut(row + x0 * 4..row + x1 * 4) {
            pixels.fill(0);
        }
    }
}

/// Close button on the overlay: the window has no taskbar entry to close from.
#[tauri::command]
fn quit_app(app_handle: AppHandle) {
    app_handle.exit(0);
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
    // Stay responsive while the game and a recorder load every core.
    vision::prefer_this_process();
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
            set_show_in_recordings,
            set_panel_rect,
            quit_app,
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
                // Hidden from capture until the user asks to appear in recordings.
                let _ = apply_capture_visibility(&window, false);
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
                let mut overlay_origin: Option<(i32, i32)> = None;
                let mut overlay_origin_checked = Instant::now() - Duration::from_secs(1);
                let mut vision_generation = state_clone.lock().vision_generation;
                let mut last_capture_status = "";
                let mut diagnostics_started = Instant::now();
                let mut diagnostic_frames = 0u32;
                let mut diagnostic_candidates = 0usize;
                let mut diagnostic_hits = 0usize;
                let mut diagnostic_ocr_ms = 0.0;
                let mut diagnostic_copy_ms = 0.0;
                let mut diagnostic_size = (0, 0);

                while running_clone.load(Ordering::Relaxed) {
                    let loop_start = Instant::now();

                    let (is_active, generation, panel) = {
                        let s = state_clone.lock();
                        let panel = if s.show_in_recordings {
                            s.panel_rect
                        } else {
                            None
                        };
                        (s.capture_active, s.vision_generation, panel)
                    };
                    // The overlay window rarely moves; ask for its position
                    // twice a second rather than every frame.
                    if panel.is_some()
                        && overlay_origin_checked.elapsed() >= Duration::from_millis(500)
                    {
                        overlay_origin_checked = Instant::now();
                        overlay_origin = app_handle
                            .get_webview_window("main")
                            .and_then(|w| w.inner_position().ok())
                            .map(|p| (p.x, p.y));
                    }
                    if generation != vision_generation {
                        vision.reset();
                        vision_generation = generation;
                    }
                    if is_active {
                        let hits = match capture.capture() {
                            CaptureOutcome::Frame(mut frame) => {
                                if let (Some(rect), Some(origin)) = (panel, overlay_origin) {
                                    mask_panel(&mut frame, origin, rect);
                                }
                                if last_capture_status != "tracking" {
                                    let _ = app_handle.emit("capture-status", "tracking");
                                    last_capture_status = "tracking";
                                }
                                let observed_at = Instant::now();
                                let analysis = vision.analyze_frame_at(&frame, observed_at);
                                diagnostic_frames += 1;
                                diagnostic_candidates += analysis.detections.len();
                                diagnostic_ocr_ms += observed_at.elapsed().as_secs_f64() * 1000.0;
                                diagnostic_copy_ms += capture.last_copy_ms();
                                diagnostic_size = (frame.width, frame.height);
                                // The pixel buffer goes back for the next frame.
                                capture.recycle(frame);
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
                            "capture_ms": if diagnostic_frames > 0 {
                                diagnostic_copy_ms / f64::from(diagnostic_frames)
                            } else {
                                0.0
                            },
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
                        diagnostic_copy_ms = 0.0;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, screen: (i32, i32)) -> capture::RawFrame {
        capture::RawFrame {
            width,
            height,
            stride: width as usize * 4,
            data: vec![255; (width * height * 4) as usize],
            screen_x: screen.0,
            screen_y: screen.1,
        }
    }

    #[test]
    fn panel_mask_blanks_only_the_panel_box() {
        let mut f = frame(200, 100, (0, 0));
        mask_panel(&mut f, (0, 0), (50, 20, 40, 30));
        let px = |x: usize, y: usize| f.data[y * f.stride + x * 4];
        assert_eq!(px(60, 30), 0);
        assert_eq!(px(44, 14), 0, "margin is covered");
        assert_eq!(px(43, 30), 255);
        assert_eq!(px(60, 57), 255);
        assert_eq!(px(150, 80), 255);
    }

    #[test]
    fn panel_mask_follows_window_and_capture_offsets_and_clips() {
        // Overlay window at (-1920, 0); the game crop starts at (-1820, 50).
        let mut f = frame(300, 200, (-1820, 50));
        mask_panel(&mut f, (-1920, 0), (110, 60, 50, 40));
        let px = |x: usize, y: usize| f.data[y * f.stride + x * 4];
        assert_eq!(px(20, 20), 0);
        assert_eq!(px(70, 20), 255);
        // Entirely outside the frame: nothing changes, nothing panics.
        let mut g = frame(100, 100, (0, 0));
        mask_panel(&mut g, (5000, 5000), (0, 0, 300, 300));
        mask_panel(&mut g, (-5000, -5000), (0, 0, 300, 300));
        assert!(g.data.iter().all(|&b| b == 255));
    }
}
