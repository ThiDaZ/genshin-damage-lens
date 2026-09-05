pub mod filter;
pub mod matcher;
pub mod tracker;

use crate::capture::RawFrame;
use filter::ColorFilter;
use matcher::DigitMatcher;
use tracker::{ConfirmedHit, HitTracker};

pub struct VisionEngine {
    matcher: DigitMatcher,
    tracker: HitTracker,
}

impl VisionEngine {
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
    /// 4. Centroid tracking & peak deduplication
    pub fn process_frame(&mut self, frame: &RawFrame) -> Vec<ConfirmedHit> {
        let components = ColorFilter::segment_frame(frame);
        let clusters = ColorFilter::cluster_components(components);

        let mut frame_detections = Vec::new();
        for cluster in &clusters {
            if let Some(hit) = self.matcher.recognize_cluster(cluster) {
                frame_detections.push((
                    hit.value,
                    cluster.element,
                    hit.is_crit,
                    hit.x + frame.screen_x,
                    hit.y + frame.screen_y,
                ));
            }
        }

        self.tracker.update(&frame_detections)
    }

    pub fn reset(&mut self) {
        self.tracker.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let comps = ColorFilter::segment_frame(&raw);
        let clusters = ColorFilter::cluster_components(comps);
        let matcher = DigitMatcher::new();

        println!("\n=== user_problem_frame.png size: {}x{} ===", width, height);
        let mut false_positives = 0;
        for (ci, cl) in clusters.iter().enumerate() {
            let res = matcher.recognize_cluster(cl);
            if let Some(hit) = res {
                false_positives += 1;
                let box_strs: Vec<_> = cl.glyphs.iter().map(|g| format!("({},{} {}x{})", g.bbox.x, g.bbox.y, g.bbox.width, g.bbox.height)).collect();
                println!("  Cluster #{}: elem={:?}, VALUE={}, conf={:.2}, crit={}, at ({},{}), glyphs: {}",
                    ci, cl.element, hit.value, hit.confidence, hit.is_crit, hit.x, hit.y, box_strs.join(" "));
            }
        }
        assert_eq!(false_positives, 0, "Expected 0 false positives in user_problem_frame.png, but found {}", false_positives);
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

            let comps = ColorFilter::segment_frame(&raw);
            let clusters = ColorFilter::cluster_components(comps);
            let mut detected = Vec::new();
            for cl in &clusters {
                if let Some(hit) = matcher.recognize_cluster(cl) {
                    detected.push((hit.value, cl.element, hit.confidence, hit.x, hit.y));
                }
            }
            total_false_hits += detected.len();
            if !detected.is_empty() {
                println!("Issue Sec {:02}: {} false hits:", sec, detected.len());
                for (v, el, conf, x, y) in detected {
                    println!("   -> value={}, elem={:?}, conf={:.2} at ({},{})", v, el, conf, x, y);
                }
            } else {
                println!("Issue Sec {:02}: clean (0 hits)", sec);
            }
        }
        assert_eq!(total_false_hits, 0, "Expected 0 false hits across issue video frames, found {}", total_false_hits);
    }

    #[test]
    fn test_recognize_frame_035_damage_4046_and_14229() {
        let frame = match load_sample_frame(35) {
            Some(f) => f,
            None => { println!("sample frame 35 missing, skipping"); return; }
        };

        let comps = ColorFilter::segment_frame(&frame);
        let clusters = ColorFilter::cluster_components(comps);
        let matcher = DigitMatcher::new();

        let mut found_4046 = false;
        let mut found_14229 = false;

        for cl in &clusters {
            if let Some(hit) = matcher.recognize_cluster(cl) {
                let box_strs: Vec<_> = cl.glyphs.iter().map(|g| format!("({},{} {}x{})", g.bbox.x, g.bbox.y, g.bbox.width, g.bbox.height)).collect();
                println!("F35 Detected: {} ({:?}, crit={}, conf={:.2}) at ({}, {}), glyphs: {}",
                    hit.value, cl.element, hit.is_crit, hit.confidence, hit.x, hit.y, box_strs.join(" "));
                if hit.value == 4046 {
                    found_4046 = true;
                }
                if hit.value == 14229 {
                    found_14229 = true;
                }
            }
        }
        assert!(found_4046, "Failed to recognize Geo 4046 in frame 35!");
        assert!(found_14229, "Failed to recognize Geo 14229 in frame 35!");
    }

    #[test]
    fn test_recognize_frame_046_damage_32625() {
        let frame = match load_sample_frame(46) {
            Some(f) => f,
            None => { println!("sample frame 46 missing, skipping"); return; }
        };

        let comps = ColorFilter::segment_frame(&frame);
        let clusters = ColorFilter::cluster_components(comps);
        let matcher = DigitMatcher::new();

        let mut found_32625 = false;
        for cl in &clusters {
            if let Some(hit) = matcher.recognize_cluster(cl) {
                println!("F46 Detected: {} ({:?}, crit={}, conf={:.2}) at ({}, {})",
                    hit.value, cl.element, hit.is_crit, hit.confidence, hit.x, hit.y);
                if hit.value == 32625 {
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
            None => { println!("sample frame 42 missing, skipping"); return; }
        };

        let comps = ColorFilter::segment_frame(&frame);
        let clusters = ColorFilter::cluster_components(comps);
        let matcher = DigitMatcher::new();



        let mut found_1141 = false;
        for cl in &clusters {
            if let Some(hit) = matcher.recognize_cluster(cl) {
                let box_strs: Vec<_> = cl.glyphs.iter().map(|g| format!("({},{} {}x{})", g.bbox.x, g.bbox.y, g.bbox.width, g.bbox.height)).collect();
                println!("F42 Detected: {} ({:?}, crit={}, conf={:.2}) at ({}, {}), glyphs: {}",
                    hit.value, cl.element, hit.is_crit, hit.confidence, hit.x, hit.y, box_strs.join(" "));
                if hit.value == 1141 {
                    found_1141 = true;
                }
            }
        }
        assert!(found_1141, "Failed to recognize Pyro 1141 in frame 42!");
    }

    #[test]
    fn test_vision_engine_tracking_end_to_end() {
        let mut engine = VisionEngine::new();
        let mut total_confirmed = 0;

        // Feed frames 30 through 45 through the vision pipeline
        for f in 30..=45 {
            if let Some(frame) = load_sample_frame(f) {
                let confirmed = engine.process_frame(&frame);
                for hit in &confirmed {
                    println!("Confirmed Hit [Frame {:02}]: value={}, elem={:?}, crit={}, at ({},{})",
                        f, hit.value, hit.element, hit.is_crit, hit.x, hit.y);
                }
                total_confirmed += confirmed.len();
            }
        }

        println!("End-to-end simulation produced {} confirmed hits across frames 30-45", total_confirmed);
        assert!(total_confirmed > 0, "Expected at least 1 confirmed hit over frames 30-45");
    }
}

