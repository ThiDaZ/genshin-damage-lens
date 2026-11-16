//! Time the capture path on its own, without the game.
//!
//! cargo run --release --example capture_bench -- [seconds] [frame.png] [--cpu]
//!
//! Captures the whole display that `capture_probe` would use and reports how
//! many frames arrived and how long the copy and read-back took. The desktop
//! only produces frames while something on it changes, so move a window or
//! play a video during the run. Pass a PNG path to save the last frame and
//! check that it is the real screen.
//!
//! Pixels are labeled on the graphics card unless `--cpu` is given or the
//! shader path is not available. Every frame's labels are then compared with
//! the labels the processor gives the same pixels, and one differing byte
//! fails the run.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use genshin_damage_lens_lib::capture::{dxgi::DxgiCapture, CaptureOutcome};
    use genshin_damage_lens_lib::vision::filter::label_bgra;
    use std::time::{Duration, Instant};

    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let on_cpu = args.iter().any(|a| a == "--cpu");
    args.retain(|a| a != "--cpu");
    let seconds: f64 = args.first().and_then(|v| v.parse().ok()).unwrap_or(5.0);
    genshin_damage_lens_lib::vision::prefer_this_process();

    let mut capture = DxgiCapture::new()?;
    capture.set_gpu_labels(!on_cpu);
    let region = capture.output_rect();
    println!(
        "Capturing {}x{} at ({},{}) on {} for {seconds} s, labels on the {}",
        region.width,
        region.height,
        region.x,
        region.y,
        capture.adapter_name(),
        match (capture.gpu_labels(), capture.gpu_labels_error()) {
            (true, _) => "graphics card".to_owned(),
            (false, Some(error)) if !on_cpu =>
                format!("processor (graphics card not available: {error})"),
            _ => "processor".to_owned(),
        }
    );
    let (mut verified, mut labeled_pixels) = (0u32, 0usize);
    let started = Instant::now();
    let (mut frames, mut waits) = (0u32, 0u32);
    let mut copy_ms = Vec::new();
    let mut last = None;
    while started.elapsed() < Duration::from_secs_f64(seconds) {
        match capture.capture_region(region)? {
            CaptureOutcome::Frame(frame) => {
                frames += 1;
                copy_ms.push(capture.last_copy().as_secs_f64() * 1000.0);
                if let Some(labels) = &frame.labels {
                    let (w, h) = (frame.width as usize, frame.height as usize);
                    let expected = label_bgra(&frame.data, frame.stride, w, h);
                    if let Some(i) = (0..expected.len().max(labels.len()))
                        .find(|&i| labels.get(i) != expected.get(i))
                    {
                        let px = &frame.data[i / w * frame.stride + i % w * 4..][..3];
                        return Err(format!(
                            "frame {frames}: label at ({},{}) is {:?} from the graphics card and \
                             {:?} from the processor, pixel r={} g={} b={}",
                            i % w,
                            i / w,
                            labels.get(i),
                            expected.get(i),
                            px[2],
                            px[1],
                            px[0]
                        )
                        .into());
                    }
                    verified += 1;
                    labeled_pixels += labels.iter().filter(|&&label| label != 0).count();
                }
                if let Some(previous) = last.replace(frame) {
                    capture.recycle(previous);
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
    if verified > 0 {
        println!(
            "labels: {verified} of {frames} frames came with labels from the graphics card, every \
             byte equal to the processor's; {} labeled pixels per frame on average",
            labeled_pixels / verified as usize
        );
    } else {
        println!("labels: none came with the frames (labeled on the processor)");
    }
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
