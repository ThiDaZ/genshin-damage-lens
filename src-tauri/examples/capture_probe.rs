//! Sample the same game crop/recognizer as the app; save a bounded local diagnosis.
use genshin_damage_lens_lib::{
    capture::{CaptureOutcome, ScreenCapture},
    vision::VisionEngine,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("Usage: capture_probe <output-dir>")?,
    );
    std::fs::create_dir_all(&output)?;
    let mut capture = ScreenCapture::new();
    let mut vision = VisionEngine::new();
    let started = Instant::now();
    let mut saved = Vec::new();
    let mut frames = 0;
    let mut total_candidates = 0;
    let mut confirmed = 0;
    let mut confirmed_events = Vec::new();
    let mut unavailable = 0;
    let mut no_frame = 0;
    let mut last_saved = None;
    while started.elapsed() < Duration::from_secs(15) {
        match capture.capture() {
            CaptureOutcome::Frame(frame) => {
                let now = Instant::now();
                let analysis = vision.analyze_frame_at(&frame, now);
                let hits = analysis.detections;
                let elapsed = now.elapsed();
                total_candidates += hits.len();
                confirmed += analysis.hits.len();
                for hit in &analysis.hits {
                    confirmed_events.push(serde_json::json!({
                        "timestamp_ms": now.duration_since(started).as_millis() as u64,
                        "value": hit.value, "element": hit.element,
                        "x": hit.x - frame.screen_x, "y": hit.y - frame.screen_y,
                    }));
                }
                frames += 1;
                println!(
                    "frame {frames}: {}x{} at ({},{}), OCR {:.1} ms, candidates {:?}",
                    frame.width,
                    frame.height,
                    frame.screen_x,
                    frame.screen_y,
                    elapsed.as_secs_f64() * 1000.0,
                    hits.iter()
                        .map(|h| (
                            h.value,
                            h.element,
                            h.confidence,
                            h.x - frame.screen_x,
                            h.y - frame.screen_y
                        ))
                        .collect::<Vec<_>>()
                );
                if saved.len() < 15
                    && last_saved.is_none_or(|last: Instant| {
                        now.duration_since(last) >= Duration::from_secs(1)
                    })
                {
                    last_saved = Some(now);
                    saved.push((now.duration_since(started).as_millis() as u64, frame));
                }
            }
            CaptureOutcome::NoNewFrame => {
                no_frame += 1;
                // A number that just vanished can still confirm here.
                let now = Instant::now();
                for hit in vision.no_new_frame(now) {
                    confirmed += 1;
                    confirmed_events.push(serde_json::json!({
                        "timestamp_ms": now.duration_since(started).as_millis() as u64,
                        "value": hit.value, "element": hit.element,
                    }));
                }
            }
            CaptureOutcome::Unavailable => {
                unavailable += 1;
                vision.reset();
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    let mut manifest = Vec::new();
    for (i, (timestamp, frame)) in saved.into_iter().enumerate() {
        let mut rgba = Vec::with_capacity((frame.width * frame.height * 4) as usize);
        for row in frame.data.chunks(frame.stride).take(frame.height as usize) {
            for px in row[..frame.width as usize * 4].chunks_exact(4) {
                rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
        }
        let name = format!("frame-{i:03}.png");
        image::save_buffer(
            output.join(&name),
            &rgba,
            frame.width,
            frame.height,
            image::ColorType::Rgba8,
        )?;
        manifest.push(serde_json::json!({"path":name,"timestamp_ms":timestamp}));
    }
    let report = serde_json::json!({"frames":manifest,"processed":frames,"candidates":total_candidates,"confirmed":confirmed,"confirmed_events":confirmed_events,"unavailable":unavailable,"timeouts":no_frame});
    std::fs::write(
        output.join("capture.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{report}");
    Ok(())
}
