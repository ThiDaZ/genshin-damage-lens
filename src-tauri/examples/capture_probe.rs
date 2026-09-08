//! Sample the same game crop/recognizer as the app; save a bounded local diagnosis.
use genshin_damage_lens_lib::{capture::{CaptureOutcome, ScreenCapture}, vision::VisionEngine};
use std::{path::PathBuf, time::{Duration, Instant}};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(std::env::args().nth(1).ok_or("Usage: capture_probe <output-dir>")?);
    std::fs::create_dir_all(&output)?;
    let mut capture = ScreenCapture::new();
    let mut vision = VisionEngine::new();
    let started = Instant::now();
    let mut saved = Vec::new();
    let mut frames = 0;
    let mut total_candidates = 0;
    let mut confirmed = 0;
    let mut unavailable = 0;
    let mut no_frame = 0;
    let mut last_saved = None;
    while started.elapsed() < Duration::from_secs(15) {
        match capture.capture() {
            CaptureOutcome::Frame(frame) => {
                let now = Instant::now();
                let hits = vision.detect_frame(&frame);
                let elapsed = now.elapsed();
                total_candidates += hits.len();
                // This diagnostic reports frame recognition; its extra work is not live FPS.
                confirmed += vision.process_frame_at(&frame, now).len();
                frames += 1;
                println!("frame {frames}: {}x{} at ({},{}), OCR {:.1} ms, candidates {:?}",
                    frame.width, frame.height, frame.screen_x, frame.screen_y,
                    elapsed.as_secs_f64()*1000.0,
                    hits.iter().map(|h| (h.value, h.element, h.confidence, h.x-frame.screen_x, h.y-frame.screen_y)).collect::<Vec<_>>());
                if saved.len() < 8 && last_saved.is_none_or(|last: Instant| now.duration_since(last) >= Duration::from_millis(750)) {
                    last_saved = Some(now);
                    saved.push((started.elapsed().as_millis() as u64, frame));
                }
            }
            CaptureOutcome::NoNewFrame => { no_frame += 1; vision.no_new_frame(Instant::now()); }
            CaptureOutcome::Unavailable => { unavailable += 1; vision.reset(); std::thread::sleep(Duration::from_millis(100)); }
        }
    }
    let mut manifest = Vec::new();
    for (i, (timestamp, frame)) in saved.into_iter().enumerate() {
        let mut rgba = Vec::with_capacity((frame.width * frame.height * 4) as usize);
        for row in frame.data.chunks(frame.stride).take(frame.height as usize) {
            for px in row[..frame.width as usize * 4].chunks_exact(4) { rgba.extend_from_slice(&[px[2],px[1],px[0],255]); }
        }
        let name = format!("frame-{i:03}.png");
        image::save_buffer(output.join(&name), &rgba, frame.width, frame.height, image::ColorType::Rgba8)?;
        manifest.push(serde_json::json!({"path":name,"timestamp_ms":timestamp}));
    }
    let report = serde_json::json!({"frames":manifest,"processed":frames,"candidates":total_candidates,"confirmed":confirmed,"unavailable":unavailable,"timeouts":no_frame});
    std::fs::write(output.join("capture.json"), serde_json::to_vec_pretty(&report)?)?;
    println!("{report}");
    Ok(())
}
