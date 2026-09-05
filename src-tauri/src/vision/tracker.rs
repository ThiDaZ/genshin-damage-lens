use crate::state::ElementType;
use std::time::Instant;

pub struct ConfirmedHit {
    pub value: u32,
    pub element: ElementType,
    pub is_crit: bool,
    pub x: i32,
    pub y: i32,
}

struct TrackedHit {
    #[allow(dead_code)]
    id: u64,
    x: f32,
    y: f32,
    initial_y: f32,
    element: ElementType,
    is_crit: bool,
    peak_value: u32,
    frames_seen: u32,
    frames_missing: u32,
    emitted: bool,
    is_static: bool,
    #[allow(dead_code)]
    created_at: Instant,
}

pub struct HitTracker {
    next_id: u64,
    active_tracks: Vec<TrackedHit>,
}

impl HitTracker {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            active_tracks: Vec::with_capacity(32),
        }
    }

    /// Update tracker with detections from current frame, returning newly confirmed peak hits
    pub fn update(
        &mut self,
        detections: &[(u32, ElementType, bool, i32, i32)],
    ) -> Vec<ConfirmedHit> {
        let mut confirmed = Vec::new();
        let mut matched_tracks = vec![false; self.active_tracks.len()];
        let mut new_tracks = Vec::new();

        for &(value, element, is_crit, x, y) in detections {
            let xf = x as f32;
            let yf = y as f32;

            let mut best_match_idx = None;
            let mut min_dist_sq = 45.0 * 45.0; // 45px association radius

            for (idx, track) in self.active_tracks.iter().enumerate() {
                if matched_tracks[idx] || track.element != element {
                    continue;
                }

                let dx = track.x - xf;
                // Genshin damage numbers float upward (y decreases)
                let dy = track.y - yf;
                let dist_sq = dx * dx + dy * dy;

                // Numbers must move upward or stay relatively stable (not jump downwards)
                if dist_sq < min_dist_sq && yf <= track.y + 10.0 {
                    min_dist_sq = dist_sq;
                    best_match_idx = Some(idx);
                }
            }

            if let Some(idx) = best_match_idx {
                matched_tracks[idx] = true;
                let track = &mut self.active_tracks[idx];
                track.x = track.x * 0.4 + xf * 0.6;
                track.y = yf;
                track.peak_value = track.peak_value.max(value);
                track.is_crit |= is_crit;
                track.frames_seen += 1;
                track.frames_missing = 0;

                let dy_up = track.initial_y - track.y;

                // Detect static UI elements or environmental scenery: if seen across 3+ frames with negligible upward drift
                if track.frames_seen >= 3 && dy_up < 1.0 {
                    track.is_static = true;
                }

                // In Genshin Impact, genuine damage numbers float upward as they animate.
                // We require 2 frames and >= 2.0px upward drift.
                if track.frames_seen >= 2 && !track.emitted && !track.is_static && dy_up >= 2.0 {
                    track.emitted = true;
                    confirmed.push(ConfirmedHit {
                        value: track.peak_value,
                        element: track.element,
                        is_crit: track.is_crit,
                        x: track.x.round() as i32,
                        y: track.y.round() as i32,
                    });
                }
            } else {
                // New track candidate
                let id = self.next_id;
                self.next_id += 1;
                new_tracks.push(TrackedHit {
                    id,
                    x: xf,
                    y: yf,
                    initial_y: yf,
                    element,
                    is_crit,
                    peak_value: value,
                    frames_seen: 1,
                    frames_missing: 0,
                    emitted: false,
                    is_static: false,
                    created_at: Instant::now(),
                });
            }
        }

        // Increment missing frames for tracks that were not detected this frame
        for (idx, &matched) in matched_tracks.iter().enumerate() {
            if !matched {
                self.active_tracks[idx].frames_missing += 1;
            }
        }

        // Append new tracks
        self.active_tracks.extend(new_tracks);

        // Flush un-emitted tracks that have at least 2 reliable observations before purging.
        self.active_tracks.retain_mut(|track| {
            if track.frames_missing > 4 {
                let dy_up = track.initial_y - track.y;
                let can_flush = !track.emitted && !track.is_static && (
                    // Standard flush
                    (track.frames_seen >= 2 && dy_up >= 2.0)
                );
                if can_flush {
                    track.emitted = true;
                    confirmed.push(ConfirmedHit {
                        value: track.peak_value,
                        element: track.element,
                        is_crit: track.is_crit,
                        x: track.x.round() as i32,
                        y: track.y.round() as i32,
                    });
                }
                false // Drop track
            } else {
                true
            }
        });

        confirmed
    }

    pub fn reset(&mut self) {
        self.active_tracks.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracker_deduplication() {
        let mut tracker = HitTracker::new();

        // Simulate a floating Pyro hit starting at y=500 and floating up to y=460 across 10 frames
        // The numbers recognized might be: 42000 (initial), 42580 (peak), 42580 (fade)
        let mut total_emitted = Vec::new();

        for frame in 0..10 {
            let y = 500 - frame * 4;
            let val = if frame == 0 { 42000 } else { 42580 };
            let hits = tracker.update(&[(val, ElementType::Pyro, true, 800, y)]);
            total_emitted.extend(hits);
        }

        // Must emit exactly 1 confirmed hit event despite 10 consecutive frames!
        assert_eq!(total_emitted.len(), 1);
        assert_eq!(total_emitted[0].value, 42580);
        assert_eq!(total_emitted[0].element, ElementType::Pyro);
        assert!(total_emitted[0].is_crit);
    }
}
