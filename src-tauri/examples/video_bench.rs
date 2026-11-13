//! Replay a recording through the live pipeline and report hits and timing.
//!
//! ffmpeg -v error -i clip.mp4 -f rawvideo -pix_fmt bgra - |
//!     cargo run --release --example video_bench -- 1920 1080 60 [--realtime] [--detections] [--trace]
//!
//! Default mode processes every frame with recording timestamps (best case).
//! `--realtime` drops the frames that would have gone by while the previous one
//! was being processed, which is what the live capture loop experiences.
use genshin_damage_lens_lib::{
    capture::RawFrame,
    vision::{tracker::ConfirmedHit, VisionEngine},
};
use std::io::Read;
use std::time::{Duration, Instant};

fn report(hit: &ConfirmedHit, t_ms: f64, hits: &mut usize, total: &mut u64) {
    *hits += 1;
    *total += u64::from(hit.value);
    println!(
        "HIT {:8.1}ms  {:>8} {:?}{} at ({},{})",
        t_ms,
        hit.value,
        hit.element,
        if hit.is_crit { " crit" } else { "" },
        hit.x,
        hit.y
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!(
            "Usage: video_bench <width> <height> <fps> [--realtime] [--detections] [--trace]"
        );
        std::process::exit(2);
    }
    let width: u32 = args[0].parse().expect("width");
    let height: u32 = args[1].parse().expect("height");
    let fps: f64 = args[2].parse().expect("fps");
    let realtime = args.iter().any(|a| a == "--realtime");
    let show_detections = args.iter().any(|a| a == "--detections");

    let frame_len = width as usize * height as usize * 4;
    let mut stdin = std::io::stdin().lock();
    let mut engine = VisionEngine::new();
    engine.set_trace(args.iter().any(|a| a == "--trace"));
    let start = Instant::now();
    let mut index = 0u64;
    let mut processed = 0u64;
    let mut busy_until_ms = 0.0f64;
    let mut times = Vec::new();
    let mut hits = 0usize;
    let mut total = 0u64;
    let mut readings = 0usize;

    loop {
        let mut data = vec![0u8; frame_len];
        if stdin.read_exact(&mut data).is_err() {
            break;
        }
        let t_ms = index as f64 * 1000.0 / fps;
        index += 1;
        if realtime && t_ms < busy_until_ms {
            continue;
        }
        let frame = RawFrame {
            width,
            height,
            stride: width as usize * 4,
            data,
            screen_x: 0,
            screen_y: 0,
        };
        let now = start + Duration::from_micros((t_ms * 1000.0) as u64);
        let began = Instant::now();
        let analysis = engine.analyze_frame_at(&frame, now);
        let ms = began.elapsed().as_secs_f64() * 1000.0;
        busy_until_ms = t_ms + ms;
        times.push(ms);
        processed += 1;
        readings += analysis.detections.len();
        if show_detections && !analysis.detections.is_empty() {
            let list: Vec<String> = analysis
                .detections
                .iter()
                .map(|d| {
                    format!(
                        "{}:{:?}:{:.2}@{},{}",
                        d.value, d.element, d.confidence, d.x, d.y
                    )
                })
                .collect();
            println!("  f{:05} {:8.1}ms  {}", index - 1, t_ms, list.join("  "));
        }
        for hit in analysis.hits {
            report(&hit, t_ms, &mut hits, &mut total);
        }
    }
    // Numbers still on screen when the recording ends.
    let end_ms = index as f64 * 1000.0 / fps;
    for hit in
        engine.no_new_frame(start + Duration::from_micros((end_ms * 1000.0) as u64 + 3_000_000))
    {
        report(&hit, end_ms, &mut hits, &mut total);
    }

    times.sort_by(f64::total_cmp);
    let mean = times.iter().sum::<f64>() / times.len().max(1) as f64;
    let pick = |q: f64| {
        times
            .get(((times.len() as f64 * q).ceil() as usize).saturating_sub(1))
            .copied()
            .unwrap_or(0.0)
    };
    println!(
        "SUMMARY frames={} processed={} readings={} hits={} total_damage={} mean_ms={:.2} p50_ms={:.2} p95_ms={:.2} max_ms={:.2}",
        index,
        processed,
        readings,
        hits,
        total,
        mean,
        pick(0.50),
        pick(0.95),
        pick(1.0)
    );
}
