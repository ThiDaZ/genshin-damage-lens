//! cargo run --release --example replay -- path/to/manifest.json [--check]
use genshin_damage_lens_lib::{
    capture::RawFrame,
    vision::{
        replay::{score, LabeledHit, ObservedHit},
        VisionEngine,
    },
};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    path: PathBuf,
    timestamp_ms: u64,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Frames,
    Events,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    mode: Mode,
    frames: Vec<Frame>,
    expected_hits: Vec<LabeledHit>,
}

fn run() -> Result<bool, Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.is_empty() || args.len() > 2 || (args.len() == 2 && args[1] != "--check") {
        return Err("Usage: replay <manifest.json> [--check]".into());
    }
    let path = Path::new(&args[0]);
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(path)?)?;
    if manifest.frames.is_empty() {
        return Err("Manifest contains no frames".into());
    }
    if manifest
        .frames
        .windows(2)
        .any(|p| p[1].timestamp_ms <= p[0].timestamp_ms)
    {
        return Err("Frame timestamps must be strictly increasing".into());
    }
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    let mut engine = VisionEngine::new();
    let start = Instant::now();
    let mut observed = Vec::new();
    let mut processing_ms = Vec::new();
    for frame in &manifest.frames {
        let image = image::open(root.join(&frame.path))?.to_rgba8();
        let (width, height) = image.dimensions();
        let mut data = image.into_raw();
        for pixel in data.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let raw = RawFrame {
            width,
            height,
            stride: width as usize * 4,
            data,
            screen_x: 0,
            screen_y: 0,
        };
        let process_start = Instant::now();
        if manifest.mode == Mode::Frames {
            for hit in engine.detect_frame(&raw) {
                observed.push(ObservedHit {
                    timestamp_ms: frame.timestamp_ms,
                    value: hit.value,
                    element: hit.element,
                    x: hit.x,
                    y: hit.y,
                });
            }
        } else {
            let now = start
                .checked_add(Duration::from_millis(frame.timestamp_ms))
                .ok_or("Timestamp out of range")?;
            for hit in engine.process_frame_at(&raw, now) {
                observed.push(ObservedHit {
                    timestamp_ms: frame.timestamp_ms,
                    value: hit.value,
                    element: hit.element,
                    x: hit.x,
                    y: hit.y,
                });
            }
        }
        processing_ms.push(process_start.elapsed().as_secs_f64() * 1000.0);
    }
    let report = score(&manifest.expected_hits, &observed)?;
    let passed = report.missed == 0 && report.false_hits == 0;
    let mean_ms = processing_ms.iter().sum::<f64>() / processing_ms.len() as f64;
    processing_ms.sort_by(f64::total_cmp);
    let p95_index = ((processing_ms.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "report": report, "observations": observed, "frame_count": manifest.frames.len(),
            "mean_processing_ms": mean_ms, "p95_processing_ms": processing_ms[p95_index],
            "note": "Processing timing excludes PNG decoding; this is not live capture FPS."
        }))?
    );
    Ok(!args.iter().any(|x| x == "--check") || passed)
}

fn main() {
    match run() {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("Replay error: {error}");
            std::process::exit(2);
        }
    }
}
