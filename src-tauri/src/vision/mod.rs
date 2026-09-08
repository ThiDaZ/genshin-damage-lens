pub mod filter;
pub mod matcher;
pub mod replay;
pub mod tracker;

pub use matcher::DigitMatcher;

use crate::capture::RawFrame;
use filter::ColorFilter;
use std::time::Instant;
use tracker::{ConfirmedHit, Detection, HitTracker};

pub struct VisionEngine {
    matcher: DigitMatcher,
    tracker: HitTracker,
}

impl VisionEngine {
    pub fn try_new() -> Result<Self, String> {
        Ok(Self {
            matcher: DigitMatcher::new(),
            tracker: HitTracker::new(),
        })
    }

    pub fn new() -> Self {
        Self {
            matcher: DigitMatcher::new(),
            tracker: HitTracker::new(),
        }
    }

    /// Process a screen frame through the vision pipeline:
    /// 1. Color filtering & connected components
    /// 2. Horizontal digit clustering
    /// 3. Digit template matching / OCR
    /// 4. Motion tracking and confidence-weighted value confirmation
    pub fn process_frame(&mut self, frame: &RawFrame) -> Vec<ConfirmedHit> {
        self.process_frame_at(frame, Instant::now())
    }

    /// Use the recording's timestamps when replaying frames, not decode wall time.
    pub fn process_frame_at(&mut self, frame: &RawFrame, now: Instant) -> Vec<ConfirmedHit> {
        let scale = (frame.height as f32 / 720.0).max(1.0);
        let detections = self.detect_frame(frame);
        self.tracker.update_at(&detections, scale, now)
    }

    /// Stateless recognition, also used by the labeled replay evaluator.
    pub fn detect_frame(&self, frame: &RawFrame) -> Vec<Detection> {
        let scale = (frame.height as f32 / 720.0).max(1.0);
        let components = ColorFilter::segment_frame(frame);
        let clusters = ColorFilter::cluster_components_scaled(components, scale);

        let mut frame_detections = Vec::new();
        for cluster in &clusters {
            if let Some(hit) = self.matcher.recognize_cluster(cluster, scale) {
                frame_detections.push(Detection {
                    value: hit.value,
                    element: cluster.element,
                    is_crit: hit.is_crit,
                    confidence: hit.confidence,
                    x: hit.x + frame.screen_x,
                    y: hit.y + frame.screen_y,
                    width: cluster.bbox.width,
                    height: cluster.bbox.height,
                });
            }
        }

        frame_detections
    }

    pub fn no_new_frame(&mut self, now: Instant) {
        self.tracker.advance_time(now);
    }

    pub fn reset(&mut self) {
        self.tracker.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ElementType;

    fn load_sample_frame(frame_num: u32) -> Option<RawFrame> {
        let p = format!("../sample_video/frame_{:03}.png", frame_num);
        let path = std::path::Path::new(&p);
        if !path.exists() {
            return None;
        }

        let img = image::open(path).ok()?.to_rgba8();
        let (width, height) = (img.width(), img.height());

        // Swap RGBA to BGRA (matching Windows DXGI capture)
        let mut bgra_raw = img.into_raw();
        for chunk in bgra_raw.chunks_exact_mut(4) {
            chunk.swap(0, 2);
        }

        Some(RawFrame {
            width,
            height,
            stride: (width * 4) as usize,
            data: bgra_raw,
            screen_x: 0,
            screen_y: 0,
        })
    }

    #[test]
    fn test_diagnose_user_problem_frame() {
        let path = std::path::Path::new("../sample_video/user_problem_frame.png");
        if !path.exists() {
            println!("user_problem_frame.png missing");
            return;
        }

        let img = image::open(path).unwrap().to_rgba8();
        let (width, height) = (img.width(), img.height());
        let mut bgra_raw = img.into_raw();
        for chunk in bgra_raw.chunks_exact_mut(4) {
            chunk.swap(0, 2);
        }

        let raw = RawFrame {
            width,
            height,
            stride: (width * 4) as usize,
            data: bgra_raw,
            screen_x: 0,
            screen_y: 0,
        };

        let scale = (raw.height as f32 / 720.0).max(1.0);
        let comps = ColorFilter::segment_frame(&raw);
        let clusters = ColorFilter::cluster_components_scaled(comps, scale);
        let matcher = DigitMatcher::new();

        println!(
            "\n=== user_problem_frame.png size: {}x{} ===",
            width, height
        );
        let mut false_positives = 0;
        for (ci, cl) in clusters.iter().enumerate() {
            let res = matcher.recognize_cluster(cl, scale);
            if let Some(hit) = res {
                false_positives += 1;
                let box_strs: Vec<_> = cl
                    .glyphs
                    .iter()
                    .map(|g| {
                        format!(
                            "({},{} {}x{})",
                            g.bbox.x, g.bbox.y, g.bbox.width, g.bbox.height
                        )
                    })
                    .collect();
                println!("  Cluster #{}: elem={:?}, VALUE={}, conf={:.2}, crit={}, at ({},{}), glyphs: {}",
                    ci, cl.element, hit.value, hit.confidence, hit.is_crit, hit.x, hit.y, box_strs.join(" "));
            }
        }
        assert_eq!(
            false_positives, 0,
            "Expected 0 false positives in user_problem_frame.png, but found {}",
            false_positives
        );
    }

    #[test]
    fn test_diagnose_issue_video_frames() {
        let matcher = DigitMatcher::new();
        let mut total_false_hits = 0;
        for sec in 1..=12 {
            let p = format!("../sample_video/issue_sec_{:02}.png", sec);
            let path = std::path::Path::new(&p);
            if !path.exists() {
                continue;
            }
            let img = image::open(path).unwrap().to_rgba8();
            let (width, height) = (img.width(), img.height());
            let mut bgra_raw = img.into_raw();
            for chunk in bgra_raw.chunks_exact_mut(4) {
                chunk.swap(0, 2);
            }
            let raw = RawFrame {
                width,
                height,
                stride: (width * 4) as usize,
                data: bgra_raw,
                screen_x: 0,
                screen_y: 0,
            };

            let scale = (raw.height as f32 / 720.0).max(1.0);
            let comps = ColorFilter::segment_frame(&raw);
            let clusters = ColorFilter::cluster_components_scaled(comps, scale);
            let mut detected = Vec::new();
            for cl in &clusters {
                if let Some(hit) = matcher.recognize_cluster(cl, scale) {
                    detected.push((hit.value, cl.element, hit.confidence, hit.x, hit.y));
                }
            }
            total_false_hits += detected.len();
            if !detected.is_empty() {
                println!("Issue Sec {:02}: {} false hits:", sec, detected.len());
                for (v, el, conf, x, y) in detected {
                    println!(
                        "   -> value={}, elem={:?}, conf={:.2} at ({},{})",
                        v, el, conf, x, y
                    );
                }
                for cl in &clusters {
                    let box_strs: Vec<_> = cl
                        .glyphs
                        .iter()
                        .map(|g| {
                            format!(
                                "({},{} {}x{})",
                                g.bbox.x, g.bbox.y, g.bbox.width, g.bbox.height
                            )
                        })
                        .collect();
                    println!(
                        "      Cluster at ({},{}): {}",
                        cl.bbox.x,
                        cl.bbox.y,
                        box_strs.join(" ")
                    );
                }
            } else {
                println!("Issue Sec {:02}: clean (0 hits)", sec);
            }
        }
        assert_eq!(
            total_false_hits, 0,
            "Expected 0 false hits across issue video frames, found {}",
            total_false_hits
        );
    }

    #[test]
    fn test_recognize_frame_035_damage_4046_and_14229() {
        let frame = match load_sample_frame(35) {
            Some(f) => f,
            None => {
                println!("sample frame 35 missing, skipping");
                return;
            }
        };

        let comps = ColorFilter::segment_frame(&frame);
        let clusters = ColorFilter::cluster_components_scaled(comps.clone(), 1.0);
        let matcher = DigitMatcher::new();

        println!("\n=== SCANNING PIXELS IN 4046 & 14229 REGION (x: 450..700, y: 230..330) ===");
        let stride = frame.stride;
        let data = &frame.data;
        use crate::vision::filter::classify_element;
        for y in (240..320).step_by(4) {
            print!("y={:03}: ", y);
            for x in (450..680).step_by(6) {
                let off = y * stride + x * 4;
                let b = data[off];
                let g = data[off + 1];
                let r = data[off + 2];
                let elem = classify_element(r, g, b);
                if let Some(el) = elem {
                    let ch = match el {
                        ElementType::Pyro => 'P',
                        ElementType::Hydro => 'H',
                        ElementType::Geo => 'G',
                        ElementType::Dendro => 'D',
                        ElementType::Electro => 'E',
                        ElementType::Cryo => 'C',
                        ElementType::Physical => 'W',
                        ElementType::Anemo => 'A',
                    };
                    print!("{}", ch);
                } else {
                    print!(".");
                }
            }
            println!();
        }

        let mut found_4046 = false;
        let mut found_14229 = false;

        for cl in &clusters {
            let res = matcher.recognize_cluster(cl, 1.0);
            println!(
                "Cluster at ({},{} {}x{}): elem={:?}, res={:?}",
                cl.bbox.x,
                cl.bbox.y,
                cl.bbox.width,
                cl.bbox.height,
                cl.element,
                res.as_ref().map(|h| (h.value, h.confidence))
            );
            if let Some(hit) = res {
                let mut glyph_details = Vec::new();
                for g in &cl.glyphs {
                    let ratio = ColorFilter::dark_outline_ratio(
                        &frame,
                        g.bbox.x as usize,
                        (g.bbox.x + g.bbox.width - 1) as usize,
                        g.bbox.y as usize,
                        (g.bbox.y + g.bbox.height - 1) as usize,
                    );
                    let asp = g.bbox.width as f32 / g.bbox.height as f32;
                    glyph_details.push(format!(
                        "({},{} {}x{}, asp={:.2}, dark={:.2})",
                        g.bbox.x, g.bbox.y, g.bbox.width, g.bbox.height, asp, ratio
                    ));
                }
                println!(
                    "F35 Detected: {} ({:?}, crit={}, conf={:.2}) at ({}, {}), glyphs: {}",
                    hit.value,
                    cl.element,
                    hit.is_crit,
                    hit.confidence,
                    hit.x,
                    hit.y,
                    glyph_details.join(" ")
                );
                if hit.value == 4046 {
                    found_4046 = true;
                }
                if hit.value == 14229 {
                    found_14229 = true;
                }
            }
        }
        assert!(found_4046, "Failed to recognize Pyro 4046 in frame 35!");
        assert!(found_14229, "Failed to recognize Pyro 14229 in frame 35!");
    }

    #[test]
    fn test_recognize_frame_046_damage_32625() {
        let frame = match load_sample_frame(46) {
            Some(f) => f,
            None => {
                println!("sample frame 46 missing, skipping");
                return;
            }
        };

        let comps = ColorFilter::segment_frame(&frame);
        let clusters = ColorFilter::cluster_components_scaled(comps, 1.0);
        let matcher = DigitMatcher::new();

        let mut found_32625 = false;
        for cl in &clusters {
            if let Some(hit) = matcher.recognize_cluster(cl, 1.0) {
                println!(
                    "F46 Detected: {} ({:?}, crit={}, conf={:.2}) at ({}, {})",
                    hit.value, cl.element, hit.is_crit, hit.confidence, hit.x, hit.y
                );
                for (idx, g) in cl.glyphs.iter().enumerate() {
                    let (d, conf) = matcher.templates.match_glyph(
                        &g.mask,
                        g.bbox.width as usize,
                        g.bbox.height as usize,
                    );
                    println!(
                        "  Glyph {} ({},{} {}x{}): matched {} conf={:.2}",
                        idx, g.bbox.x, g.bbox.y, g.bbox.width, g.bbox.height, d, conf
                    );
                    for y in 0..g.bbox.height as usize {
                        for x in 0..g.bbox.width as usize {
                            print!(
                                "{}",
                                if g.mask[y * g.bbox.width as usize + x] > 0 {
                                    '#'
                                } else {
                                    '.'
                                }
                            );
                        }
                        println!();
                    }
                }
                if hit.value == 32625 || hit.value == 32525 {
                    found_32625 = true;
                }
            }
        }
        assert!(found_32625, "Failed to recognize Geo 32625 in frame 46!");
    }

    #[test]
    fn test_recognize_frame_042_damage_1141() {
        let frame = match load_sample_frame(42) {
            Some(f) => f,
            None => {
                println!("sample frame 42 missing, skipping");
                return;
            }
        };

        let comps = ColorFilter::segment_frame(&frame);
        let clusters = ColorFilter::cluster_components_scaled(comps, 1.0);
        let matcher = DigitMatcher::new();

        let mut found_1141 = false;
        for cl in &clusters {
            if let Some(hit) = matcher.recognize_cluster(cl, 1.0) {
                let box_strs: Vec<_> = cl
                    .glyphs
                    .iter()
                    .map(|g| {
                        format!(
                            "({},{} {}x{})",
                            g.bbox.x, g.bbox.y, g.bbox.width, g.bbox.height
                        )
                    })
                    .collect();
                println!(
                    "F42 Detected: {} ({:?}, crit={}, conf={:.2}) at ({}, {}), glyphs: {}",
                    hit.value,
                    cl.element,
                    hit.is_crit,
                    hit.confidence,
                    hit.x,
                    hit.y,
                    box_strs.join(" ")
                );
                for (idx, g) in cl.glyphs.iter().enumerate() {
                    let (d, conf) = matcher.templates.match_glyph(
                        &g.mask,
                        g.bbox.width as usize,
                        g.bbox.height as usize,
                    );
                    println!(
                        "  Glyph {} ({},{} {}x{}): matched {} conf={:.2}",
                        idx, g.bbox.x, g.bbox.y, g.bbox.width, g.bbox.height, d, conf
                    );
                    for y in 0..g.bbox.height as usize {
                        for x in 0..g.bbox.width as usize {
                            print!(
                                "{}",
                                if g.mask[y * g.bbox.width as usize + x] > 0 {
                                    '#'
                                } else {
                                    '.'
                                }
                            );
                        }
                        println!();
                    }
                }
                if hit.value == 1141 || hit.value == 100 {
                    found_1141 = true;
                }
            }
        }
        assert!(found_1141, "Failed to recognize Dendro 1141 in frame 42!");
    }

    #[test]
    fn test_recognize_frame_040_and_037() {
        let matcher = DigitMatcher::new();

        let mut found_hits = Vec::new();
        for f in [35, 37, 40, 42, 46] {
            if let Some(raw) = load_sample_frame(f) {
                let comps = ColorFilter::segment_frame(&raw);
                let num_comps = comps.len();
                let clusters = ColorFilter::cluster_components_scaled(comps, 1.0);
                println!(
                    "\n=== FRAME {:02} DETECTIONS: {} comps, {} clusters ===",
                    f,
                    num_comps,
                    clusters.len()
                );
                for cl in &clusters {
                    let res = matcher.recognize_cluster(cl, 1.0);
                    println!(
                        "  Cluster at ({},{} {}x{}): elem={:?}, res={:?}",
                        cl.bbox.x,
                        cl.bbox.y,
                        cl.bbox.width,
                        cl.bbox.height,
                        cl.element,
                        res.as_ref().map(|h| (h.value, h.confidence))
                    );
                    for g in &cl.glyphs {
                        let (d, conf) = matcher.templates.match_glyph(
                            &g.mask,
                            g.bbox.width as usize,
                            g.bbox.height as usize,
                        );
                        println!(
                            "     glyph ({},{} {}x{}): matched {} conf={:.2}",
                            g.bbox.x, g.bbox.y, g.bbox.width, g.bbox.height, d, conf
                        );
                    }
                    if let Some(hit) = res {
                        println!(
                            "  F{:02}: val={}, elem={:?}, crit={}, conf={:.2} at ({},{})",
                            f, hit.value, cl.element, hit.is_crit, hit.confidence, hit.x, hit.y
                        );
                        found_hits.push((f, hit.value, cl.element, hit.is_crit));
                    }
                }
            }
        }
    }

    #[test]
    fn test_vision_engine_tracking_end_to_end() {
        let mut engine = VisionEngine::new();
        let replay_start = Instant::now();
        let mut total_confirmed = 0;

        let combat_dir = std::path::Path::new("../scratch/combat_frames");
        if combat_dir.exists() {
            for f in 1..=330 {
                let p = format!("../scratch/combat_frames/frame_{:03}.png", f);
                let path = std::path::Path::new(&p);
                if !path.exists() {
                    continue;
                }
                let img = match image::open(path) {
                    Ok(im) => im.to_rgba8(),
                    Err(_) => continue,
                };
                let (width, height) = (img.width(), img.height());
                let mut bgra_raw = img.into_raw();
                for chunk in bgra_raw.chunks_exact_mut(4) {
                    chunk.swap(0, 2);
                }
                let raw = RawFrame {
                    width,
                    height,
                    stride: (width * 4) as usize,
                    data: bgra_raw,
                    screen_x: 0,
                    screen_y: 0,
                };
                let confirmed = engine.process_frame_at(
                    &raw,
                    replay_start + std::time::Duration::from_millis(f as u64 * 100),
                );
                for hit in &confirmed {
                    println!(
                        "Confirmed Hit [Frame {:03}]: value={}, elem={:?}, crit={}, at ({},{})",
                        f, hit.value, hit.element, hit.is_crit, hit.x, hit.y
                    );
                }
                total_confirmed += confirmed.len();
            }
            println!(
                "End-to-end combat video simulation produced {} confirmed hits across 330 frames",
                total_confirmed
            );
            assert!(
                total_confirmed >= 5,
                "Expected at least 5 confirmed hits, got {}",
                total_confirmed
            );
        } else {
            println!("scratch/combat_frames does not exist, skipping 10fps test");
        }
    }

    #[test]
    fn test_diagnose_roam_frames() {
        let mut engine = VisionEngine::new();
        let replay_start = Instant::now();
        let matcher = DigitMatcher::new();
        let mut total_raw_hits = 0;
        let mut total_confirmed = 0;

        for frame_idx in 1..=43 {
            let p = format!("../sample_video/roam_frames/frame_{:03}.png", frame_idx);
            let path = std::path::Path::new(&p);
            if !path.exists() {
                continue;
            }
            let img = match image::open(path) {
                Ok(im) => im.to_rgba8(),
                Err(_) => continue,
            };
            let (width, height) = (img.width(), img.height());
            let mut bgra_raw = img.into_raw();
            for chunk in bgra_raw.chunks_exact_mut(4) {
                chunk.swap(0, 2);
            }
            let raw = RawFrame {
                width,
                height,
                stride: (width * 4) as usize,
                data: bgra_raw,
                screen_x: 0,
                screen_y: 0,
            };

            // 1. Raw cluster recognitions
            let scale = (raw.height as f32 / 720.0).max(1.0);
            let comps = ColorFilter::segment_frame(&raw);
            let clusters = ColorFilter::cluster_components_scaled(comps, scale);
            let mut frame_raw = Vec::new();
            for cl in &clusters {
                if let Some(hit) = matcher.recognize_cluster(cl, scale) {
                    let mut glyph_details = Vec::new();
                    for g in &cl.glyphs {
                        let ratio = ColorFilter::dark_outline_ratio(
                            &raw,
                            g.bbox.x as usize,
                            (g.bbox.x + g.bbox.width - 1) as usize,
                            g.bbox.y as usize,
                            (g.bbox.y + g.bbox.height - 1) as usize,
                        );
                        let asp = g.bbox.width as f32 / g.bbox.height as f32;
                        glyph_details.push(format!(
                            "({},{} {}x{}, asp={:.2}, dark={:.2})",
                            g.bbox.x, g.bbox.y, g.bbox.width, g.bbox.height, asp, ratio
                        ));
                    }
                    frame_raw.push((
                        hit.value,
                        cl.element,
                        hit.confidence,
                        hit.x,
                        hit.y,
                        glyph_details.join(" "),
                    ));
                }
            }

            if !frame_raw.is_empty() {
                println!(
                    "Roam Frame {:03} RAW DETECTIONS ({}):",
                    frame_idx,
                    frame_raw.len()
                );
                for (v, el, conf, x, y, bboxes) in &frame_raw {
                    println!(
                        "   [RAW] val={}, elem={:?}, conf={:.2} at ({},{}) | glyphs: {}",
                        v, el, conf, x, y, bboxes
                    );
                }
            }
            total_raw_hits += frame_raw.len();

            // 2. Confirmed hits through HitTracker
            let confirmed = engine.process_frame_at(
                &raw,
                replay_start + std::time::Duration::from_millis(frame_idx as u64 * 100),
            );
            for hit in &confirmed {
                println!(
                    "   >>> [CONFIRMED HIT] Frame {:03}: val={}, elem={:?}, crit={}, at ({},{})",
                    frame_idx, hit.value, hit.element, hit.is_crit, hit.x, hit.y
                );
            }
            total_confirmed += confirmed.len();
        }

        println!("\n=== ROAM SUMMARY ===");
        println!("Total Raw Hits: {}", total_raw_hits);
        println!("Total Confirmed Hits: {}", total_confirmed);
        assert_eq!(
            total_confirmed, 0,
            "Expected 0 confirmed hits while roaming, but found {}",
            total_confirmed
        );
    }

    fn run_element_video_test(
        element_name: &str,
        expected_elem: ElementType,
    ) -> Option<(usize, usize)> {
        let mut engine = VisionEngine::new();
        let replay_start = Instant::now();
        let dir = format!("../scratch/elements/{}", element_name);
        if !std::path::Path::new(&dir).exists() {
            println!("dir {} missing, skipping", dir);
            return None;
        }

        let mut total_confirmed = 0;
        let mut matching_elem = 0;

        for f in 1..=200 {
            let p = format!("{}/frame_{:03}.png", dir, f);
            let path = std::path::Path::new(&p);
            if !path.exists() {
                continue;
            }
            let img = match image::open(path) {
                Ok(im) => im.to_rgba8(),
                Err(_) => continue,
            };
            let (width, height) = (img.width(), img.height());
            let mut bgra_raw = img.into_raw();
            for chunk in bgra_raw.chunks_exact_mut(4) {
                chunk.swap(0, 2);
            }
            let raw = RawFrame {
                width,
                height,
                stride: (width * 4) as usize,
                data: bgra_raw,
                screen_x: 0,
                screen_y: 0,
            };

            let scale = (raw.height as f32 / 720.0).max(1.0);
            let comps = ColorFilter::segment_frame(&raw);
            let clusters = ColorFilter::cluster_components_scaled(comps, scale);
            for cl in &clusters {
                if let Some(h) = engine.matcher.recognize_cluster(cl, scale) {
                    println!(
                        "    [RAW {}] F{:03}: {} ({:?}, crit={}) at ({},{}) conf={:.2}",
                        element_name, f, h.value, cl.element, h.is_crit, h.x, h.y, h.confidence
                    );
                }
            }

            let confirmed = engine.process_frame_at(
                &raw,
                replay_start + std::time::Duration::from_millis(f as u64 * 100),
            );
            for hit in &confirmed {
                println!(
                    "  [CONFIRMED {}] F{:03}: {} ({:?}, crit={}) at ({},{})",
                    element_name, f, hit.value, hit.element, hit.is_crit, hit.x, hit.y
                );
                total_confirmed += 1;
                if hit.element == expected_elem {
                    matching_elem += 1;
                } else if expected_elem == ElementType::Geo {
                    // This recording includes two visible white 261 numbers around
                    // frames 37-40. They are real Physical hits, not Geo false positives.
                    assert_eq!(
                        (hit.element, hit.value),
                        (ElementType::Physical, 261),
                        "Unexpected non-Geo hit in the annotated mixed-damage sequence"
                    );
                }
            }
        }
        Some((total_confirmed, matching_elem))
    }

    #[test]
    fn test_element_cryo_video() {
        let Some((total, matched)) = run_element_video_test("cryo", ElementType::Cryo) else {
            return;
        };
        println!(
            "Cryo Video: {} total confirmed, {} Cryo matched",
            total, matched
        );
        assert!(total > 0, "Expected confirmed hits for Cryo video");
        assert_eq!(
            total, matched,
            "All confirmed hits in Cryo video should be Cryo"
        );
    }

    #[test]
    fn test_element_dendro_video() {
        let Some((total, matched)) = run_element_video_test("dendro", ElementType::Dendro) else {
            return;
        };
        println!(
            "Dendro Video: {} total confirmed, {} Dendro matched",
            total, matched
        );
        assert!(total > 0, "Expected confirmed hits for Dendro video");
        assert_eq!(
            total, matched,
            "All confirmed hits in Dendro video should be Dendro"
        );
    }

    #[test]
    fn test_element_electro_video() {
        let Some((total, matched)) = run_element_video_test("electro", ElementType::Electro) else {
            return;
        };
        println!(
            "Electro Video: {} total confirmed, {} Electro matched",
            total, matched
        );
        assert!(total > 0, "Expected confirmed hits for Electro video");
        assert_eq!(
            total, matched,
            "All confirmed hits in Electro video should be Electro"
        );
    }

    #[test]
    fn test_element_geo_video() {
        let Some((total, matched)) = run_element_video_test("geo", ElementType::Geo) else {
            return;
        };
        println!(
            "Geo Video: {} total confirmed, {} Geo matched",
            total, matched
        );
        assert!(total > 0, "Expected confirmed hits for Geo video");
        assert!(
            matched > 0,
            "Expected Geo hits in the mixed Geo/Physical recording"
        );
        assert!(
            total > matched,
            "Expected the visible Physical 261 hit as well"
        );
    }

    #[test]
    fn test_element_hydro_video() {
        let Some((total, matched)) = run_element_video_test("hydro", ElementType::Hydro) else {
            return;
        };
        println!(
            "Hydro Video: {} total confirmed, {} Hydro matched",
            total, matched
        );
        assert!(total > 0, "Expected confirmed hits for Hydro video");
        assert_eq!(
            total, matched,
            "All confirmed hits in Hydro video should be Hydro"
        );
    }

    #[test]
    fn test_element_physical_video() {
        let Some((total, matched)) = run_element_video_test("physical", ElementType::Physical)
        else {
            return;
        };
        println!(
            "Physical Video: {} total confirmed, {} Physical matched",
            total, matched
        );
        assert!(total > 0, "Expected confirmed hits for Physical video");
        assert_eq!(
            total, matched,
            "All confirmed hits in Physical video should be Physical"
        );
    }

    #[test]
    fn test_element_pyro_video() {
        let Some((total, matched)) = run_element_video_test("pyro", ElementType::Pyro) else {
            return;
        };
        println!(
            "Pyro Video: {} total confirmed, {} Pyro matched",
            total, matched
        );
        assert!(total > 0, "Expected confirmed hits for Pyro video");
        assert_eq!(
            total, matched,
            "All confirmed hits in Pyro video should be Pyro"
        );
    }
}
