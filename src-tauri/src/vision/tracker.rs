use crate::state::ElementType;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// One OCR observation in screen coordinates, before temporal confirmation.
#[derive(Debug, Clone, Copy)]
pub struct Detection {
    pub value: u32,
    pub element: ElementType,
    pub is_crit: bool,
    pub confidence: f32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfirmedHit {
    pub value: u32,
    pub element: ElementType,
    pub is_crit: bool,
    pub x: i32,
    pub y: i32,
}

const TRACK_GAP: Duration = Duration::from_millis(350);
const CONFIRM_DELAY: Duration = Duration::from_millis(65);
const VOTE_WINDOW: usize = 12;

struct TrackedHit {
    last: Detection,
    initial_y: i32,
    velocity: (f32, f32),
    created_at: Instant,
    last_seen: Instant,
    observations: VecDeque<Detection>,
    emitted: bool,
}

impl TrackedHit {
    fn new(detection: Detection, now: Instant) -> Self {
        Self {
            last: detection,
            initial_y: detection.y,
            velocity: (0.0, 0.0),
            created_at: now,
            last_seen: now,
            observations: VecDeque::from([detection]),
            emitted: false,
        }
    }

    fn observe(&mut self, detection: Detection, now: Instant) {
        let dt = now.saturating_duration_since(self.last_seen).as_secs_f32();
        if dt > 0.0 {
            self.velocity.0 = (self.velocity.0 * 0.5
                + (detection.x - self.last.x) as f32 / dt * 0.5)
                .clamp(-500.0, 500.0);
            self.velocity.1 = (self.velocity.1 * 0.5
                + (detection.y - self.last.y) as f32 / dt * 0.5)
                .clamp(-500.0, 500.0);
        }
        self.last = detection;
        self.last_seen = now;
        self.observations.push_back(detection);
        if self.observations.len() > VOTE_WINDOW {
            self.observations.pop_front();
        }
    }

    fn confirm(&mut self, now: Instant, scale: f32) -> Option<ConfirmedHit> {
        if self.emitted
            || self.observations.len() < 3
            || now.saturating_duration_since(self.created_at) < CONFIRM_DELAY
            || ((self.initial_y - self.last.y) as f32) < 2.0 * scale
        {
            return None;
        }

        // Vote for a complete observed value, never the numeric maximum.
        let total_weight: f32 = self.observations.iter().map(|d| d.confidence).sum();
        let mut best = None;
        let mut best_weight = 0.0;
        for candidate in &self.observations {
            let mut count = 0;
            let mut weight = 0.0;
            let mut crit_weight = 0.0;
            for d in &self.observations {
                if d.value == candidate.value && d.element == candidate.element {
                    count += 1;
                    weight += d.confidence;
                    if d.is_crit {
                        crit_weight += d.confidence;
                    }
                }
            }
            if count >= 2 && weight > best_weight && weight >= total_weight * 0.60 {
                best = Some(ConfirmedHit {
                    value: candidate.value,
                    element: candidate.element,
                    is_crit: crit_weight > weight * 0.5,
                    x: self.last.x,
                    y: self.last.y,
                });
                best_weight = weight;
            }
        }
        self.emitted = best.is_some();
        best
    }
}

pub struct HitTracker {
    active_tracks: Vec<TrackedHit>,
    last_update: Option<Instant>,
}

impl Default for HitTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl HitTracker {
    pub fn new() -> Self {
        Self {
            active_tracks: Vec::with_capacity(32),
            last_update: None,
        }
    }

    pub fn update(&mut self, detections: &[Detection], scale: f32) -> Vec<ConfirmedHit> {
        self.update_at(detections, scale, Instant::now())
    }

    /// Explicit timestamps make video replays independent of decoding speed.
    pub fn update_at(
        &mut self,
        detections: &[Detection],
        scale: f32,
        now: Instant,
    ) -> Vec<ConfirmedHit> {
        if self.last_update.is_some_and(|last| now < last) {
            self.reset();
        }
        self.last_update = Some(now);
        let scale = if scale.is_finite() {
            scale.max(1.0)
        } else {
            1.0
        };
        self.active_tracks
            .retain(|t| now.saturating_duration_since(t.last_seen) <= TRACK_GAP);
        let valid: Vec<_> = detections
            .iter()
            .copied()
            .filter(|d| {
                (10..=99_999_999).contains(&d.value)
                    && d.confidence.is_finite()
                    && d.confidence >= 0.50
                    && d.width > 0
                    && d.height > 0
            })
            .map(|mut d| {
                d.confidence = d.confidence.min(1.0);
                d
            })
            .collect();

        // Global greedy assignment: a track and detection can each be used only once.
        let mut pairs = Vec::new();
        for (ti, track) in self.active_tracks.iter().enumerate() {
            let dt = now.saturating_duration_since(track.last_seen).as_secs_f32();
            let px = track.last.x as f32 + track.velocity.0 * dt;
            let py = track.last.y as f32 + track.velocity.1 * dt;
            for (di, d) in valid.iter().enumerate() {
                let hr = d.height as f32 / track.last.height as f32;
                let wr = d.width as f32 / track.last.width as f32;
                if d.element != track.last.element
                    || !(0.55..=1.8).contains(&hr)
                    || !(0.45..=2.2).contains(&wr)
                    || d.y as f32 > track.last.y as f32 + 12.0 * scale
                {
                    continue;
                }
                let dx = d.x as f32 - px;
                let dy = d.y as f32 - py;
                let distance = (dx * dx + dy * dy).sqrt();
                if distance > 32.0 * scale {
                    continue;
                }
                let value_penalty = if track.last.value == d.value {
                    0.0
                } else {
                    8.0 * scale
                };
                pairs.push((
                    distance + value_penalty + hr.ln().abs() * 8.0 * scale,
                    ti,
                    di,
                ));
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut used_tracks = vec![false; self.active_tracks.len()];
        let mut used_detections = vec![false; valid.len()];
        let mut confirmed = Vec::new();
        for (_, ti, di) in pairs {
            if used_tracks[ti] || used_detections[di] {
                continue;
            }
            used_tracks[ti] = true;
            used_detections[di] = true;
            let track = &mut self.active_tracks[ti];
            track.observe(valid[di], now);
            if let Some(hit) = track.confirm(now, scale) {
                confirmed.push(hit);
            }
        }
        for (di, d) in valid.into_iter().enumerate() {
            if !used_detections[di] {
                self.active_tracks.push(TrackedHit::new(d, now));
            }
        }
        confirmed
    }

    /// A timeout ages tracks but is never a new OCR observation.
    pub fn advance_time(&mut self, now: Instant) {
        if self.last_update.is_some_and(|last| now < last) {
            self.reset();
        }
        self.last_update = Some(now);
        self.active_tracks
            .retain(|t| now.saturating_duration_since(t.last_seen) <= TRACK_GAP);
    }

    pub fn reset(&mut self) {
        self.active_tracks.clear();
        self.last_update = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detection(value: u32, x: i32, y: i32) -> Detection {
        Detection {
            value,
            element: ElementType::Pyro,
            is_crit: true,
            confidence: 0.85,
            x,
            y,
            width: 80,
            height: 30,
        }
    }

    #[test]
    fn consensus_rejects_larger_ocr_outlier_and_emits_once() {
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        let mut hits = Vec::new();
        for (i, value) in [14229, 74229, 14229, 14229, 14229, 14229]
            .into_iter()
            .enumerate()
        {
            hits.extend(tracker.update_at(
                &[detection(value, 800, 500 - i as i32 * 3)],
                1.0,
                start + Duration::from_millis(i as u64 * 35),
            ));
        }
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].value, 14229);
    }

    #[test]
    fn timeout_preserves_track_without_adding_observations() {
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        assert!(tracker
            .update_at(&[detection(1000, 800, 500)], 1.0, start)
            .is_empty());
        for ms in [16, 32, 48, 64] {
            tracker.advance_time(start + Duration::from_millis(ms));
        }
        assert!(tracker
            .update_at(
                &[detection(1000, 800, 497)],
                1.0,
                start + Duration::from_millis(80)
            )
            .is_empty());
        assert_eq!(
            tracker
                .update_at(
                    &[detection(1000, 800, 494)],
                    1.0,
                    start + Duration::from_millis(115)
                )
                .len(),
            1
        );
    }

    #[test]
    fn static_text_never_confirms_and_expires() {
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        for i in 0..10 {
            assert!(tracker
                .update_at(
                    &[detection(1000, 800, 500)],
                    1.0,
                    start + Duration::from_millis(i * 50)
                )
                .is_empty());
        }
        tracker.advance_time(start + Duration::from_secs(2));
        assert!(tracker.active_tracks.is_empty());
    }

    #[test]
    fn nearby_hits_survive_reordered_detections() {
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        let mut hits = Vec::new();
        for i in 0..5 {
            let mut ds = vec![
                detection(1000, 800, 500 - i * 3),
                detection(2000, 820, 500 - i * 3),
            ];
            if i % 2 == 1 {
                ds.reverse();
            }
            hits.extend(tracker.update_at(&ds, 1.0, start + Duration::from_millis(i as u64 * 35)));
        }
        hits.sort_by_key(|h| h.value);
        assert_eq!(
            hits.iter().map(|h| h.value).collect::<Vec<_>>(),
            vec![1000, 2000]
        );
    }

    #[test]
    fn reset_and_long_gap_require_new_evidence() {
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        tracker.update_at(&[detection(1000, 800, 500)], 1.0, start);
        tracker.update_at(
            &[detection(1000, 800, 497)],
            1.0,
            start + Duration::from_millis(35),
        );
        tracker.reset();
        assert!(tracker
            .update_at(
                &[detection(1000, 800, 494)],
                1.0,
                start + Duration::from_millis(70)
            )
            .is_empty());
        assert!(tracker
            .update_at(
                &[detection(1000, 800, 491)],
                1.0,
                start + Duration::from_secs(1)
            )
            .is_empty());
        assert_eq!(tracker.active_tracks[0].observations.len(), 1);
    }

    #[test]
    fn disagreement_and_invalid_confidence_do_not_confirm() {
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        for i in 0..5 {
            let mut d = detection(1000 + i as u32, 800, 500 - i * 3);
            if i == 2 {
                d.confidence = f32::NAN;
            }
            assert!(tracker
                .update_at(&[d], 1.0, start + Duration::from_millis(i as u64 * 35))
                .is_empty());
        }
    }

    #[test]
    fn test_tracker_deduplication() {
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        let mut hits = Vec::new();
        for i in 0..10 {
            let value = if i == 0 { 42000 } else { 42580 };
            hits.extend(tracker.update_at(
                &[detection(value, 800, 500 - i * 4)],
                1.0,
                start + Duration::from_millis(i as u64 * 35),
            ));
        }
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].value, 42580);
        assert_eq!(hits[0].element, ElementType::Pyro);
        assert!(hits[0].is_crit);
    }
}
