//! Time the capture path on its own, without the game.
//!
//! cargo run --release --example capture_bench -- [seconds] [frame.png]
//!
//! Captures the whole display that `capture_probe` would use and reports how
//! many frames arrived and how long the copy and read-back took. The desktop
//! only produces frames while something on it changes, so move a window or
//! play a video during the run. Pass a PNG path to save the last frame and
//! check that it is the real screen.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use genshin_damage_lens_lib::capture::{dxgi::DxgiCapture, CaptureOutcome};
    use std::time::{Duration, Instant};

    let args: Vec<String> = std::env::args().skip(1).collect();
    let seconds: f64 = args.first().and_then(|v| v.parse().ok()).unwrap_or(5.0);
    genshin_damage_lens_lib::vision::prefer_this_process();

    let mut capture = DxgiCapture::new()?;
    let region = capture.output_rect();
    println!(
        "Capturing {}x{} at ({},{}) for {seconds} s",
        region.width, region.height, region.x, region.y
    );
    let started = Instant::now();
    let (mut frames, mut waits) = (0u32, 0u32);
    let mut copy_ms = Vec::new();
    let mut last = None;
    while started.elapsed() < Duration::from_secs_f64(seconds) {
        match capture.capture_region(region)? {
            CaptureOutcome::Frame(frame) => {
                frames += 1;
                copy_ms.push(capture.last_copy().as_secs_f64() * 1000.0);
                if let Some(previous) = last.replace(frame) {
                    capture.recycle(previous.data);
                }
            }
            CaptureOutcome::NoNewFrame => waits += 1,
            CaptureOutcome::Unavailable => return Err("capture region unavailable".into()),
        }
    }
    copy_ms.sort_by(f64::total_cmp);
    let pick = |q: f64| {
        copy_ms
            .get(((copy_ms.len() as f64 * q).ceil() as usize).saturating_sub(1))
            .copied()
            .unwrap_or(0.0)
    };
    println!(
        "frames={frames} ({:.1} fps) waits={waits} copy_ms mean={:.2} p50={:.2} p95={:.2} max={:.2}",
        f64::from(frames) / started.elapsed().as_secs_f64(),
        copy_ms.iter().sum::<f64>() / copy_ms.len().max(1) as f64,
        pick(0.5),
        pick(0.95),
        pick(1.0),
    );
    if let Some(frame) = last {
        let distinct: std::collections::HashSet<&[u8]> =
            frame.data.chunks_exact(4).step_by(997).collect();
        println!("last frame: {} distinct sampled colors", distinct.len());
        if let Some(path) = args.get(1) {
            let mut rgba = Vec::with_capacity(frame.data.len());
            for px in frame.data.chunks_exact(4) {
                rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
            image::save_buffer(
                path,
                &rgba,
                frame.width,
                frame.height,
                image::ColorType::Rgba8,
            )?;
            println!("saved {path}");
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("capture_bench needs Windows.");
}
