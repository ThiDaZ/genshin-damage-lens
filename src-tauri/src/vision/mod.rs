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
    /// 2. Digit template matching / OCR
    /// 3. Centroid tracking & peak deduplication
    pub fn process_frame(&mut self, frame: &RawFrame) -> Vec<ConfirmedHit> {
        let components = ColorFilter::segment_frame(frame);

        let mut frame_detections = Vec::new();
        for comp in &components {
            if let Some(hit) = self.matcher.recognize(comp) {
                frame_detections.push((
                    hit.value,
                    comp.element,
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
