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

/// A track survives this long without a matching reading (brief occlusion).
const TRACK_GAP: Duration = Duration::from_millis(450);
/// Readings are collected this long before the vote. The first frames of a
/// number are the least reliable: it pops in oversized, half transparent and
/// often on top of the previous number.
const CONFIRM_DELAY: Duration = Duration::from_millis(600);
/// A damage number is on screen for roughly 1.3 s. A confirmed track is kept
/// that long so a number hidden by an effect is not counted again on return.
const NUMBER_LIFETIME: Duration = Duration::from_millis(1600);
/// Shortest time a number must be seen to count when it vanishes early.
const MIN_VISIBLE: Duration = Duration::from_millis(60);
/// Text that never moves or resizes for this long is HUD, not damage.
const STATIC_LIMIT: Duration = Duration::from_millis(2500);
const VOTE_WINDOW: usize = 64;

fn digits_contain(whole: u32, part: u32) -> bool {
    whole != part && whole.to_string().contains(&part.to_string())
}

/// Could `b` be a reading of the number `a`? The same value, a fragment or a
/// longer reading of it, or the same length with a single digit misread.
fn digits_related(a: u32, b: u32) -> bool {
    if a == b || digits_contain(a, b) || digits_contain(b, a) {
        return true;
    }
    let (a, b) = (a.to_string(), b.to_string());
    a.len() == b.len() && a.bytes().zip(b.bytes()).filter(|(x, y)| x != y).count() <= 1
}

fn center(d: &Detection) -> (f32, f32) {
    (d.x as f32, d.y as f32 + d.height as f32 * 0.5)
}

struct TrackedHit {
    last: Detection,
    first: Detection,
    velocity: (f32, f32),
    created_at: Instant,
    last_seen: Instant,
    /// Last time any evidence (including a partial reading) supported the track.
    last_support: Instant,
    observations: VecDeque<Detection>,
    moved: bool,
    /// Frames processed while the number was being read, and readings made.
    first_frame: u64,
    last_frame: u64,
    readings: u32,
    emitted: bool,
    /// The confirmed value, kept to recognise the same number coming back.
    emitted_value: Option<u32>,
    /// Confirmed this frame, not yet handed out.
    pending: Option<ConfirmedHit>,
    suppressed: bool,
}

impl TrackedHit {
    fn new(detection: Detection, now: Instant, frame: u64) -> Self {
        Self {
            last: detection,
            first: detection,
            velocity: (0.0, 0.0),
            created_at: now,
            last_seen: now,
            last_support: now,
            observations: VecDeque::from([detection]),
            moved: false,
            first_frame: frame,
            last_frame: frame,
            readings: 1,
            emitted: false,
            emitted_value: None,
            pending: None,
            suppressed: false,
        }
    }

    fn predicted_center(&self, now: Instant) -> (f32, f32) {
        let dt = now
            .saturating_duration_since(self.last_seen)
            .as_secs_f32()
            .min(0.25);
        let (cx, cy) = center(&self.last);
        (cx + self.velocity.0 * dt, cy + self.velocity.1 * dt)
    }

    fn observe(&mut self, detection: Detection, now: Instant, scale: f32, frame: u64) {
        self.last_frame = frame;
        self.readings += 1;
        let dt = now.saturating_duration_since(self.last_seen).as_secs_f32();
        if dt > 0.0 {
            let (ax, ay) = center(&self.last);
            let (bx, by) = center(&detection);
            self.velocity.0 = (self.velocity.0 * 0.5 + (bx - ax) / dt * 0.5).clamp(-900.0, 900.0);
            self.velocity.1 = (self.velocity.1 * 0.5 + (by - ay) / dt * 0.5).clamp(-900.0, 900.0);
        }
        // Damage numbers pop in large, shrink and follow the camera in any
        // direction. HUD text does none of that.
        let (fx, fy) = center(&self.first);
        let (cx, cy) = center(&detection);
        let size = detection.height as f32 / self.first.height.max(1) as f32;
        if (cx - fx).abs().max((cy - fy).abs()) >= 3.0 * scale || !(0.88..=1.14).contains(&size) {
            self.moved = true;
        }
        self.last = detection;
        self.last_seen = now;
        self.last_support = now;
        self.observations.push_back(detection);
        if self.observations.len() > VOTE_WINDOW {
            self.observations.pop_front();
        }
    }

    /// Confidence-weighted vote. Readings of one number differ mostly in single
    /// digits (a 3 seen as 8 while small, a 1 as 9 while popping in), so the
    /// digits are voted per position among readings of the dominant length
    /// instead of picking one whole reading.
    fn vote(&self, min_count: usize, min_share: f32) -> Option<(u32, bool)> {
        let length = |value: u32| value.to_string().len();
        let total: f32 = self.observations.iter().map(|d| d.confidence).sum();
        let mut weight = [0.0f32; 11];
        let mut count = [0usize; 11];
        for d in &self.observations {
            weight[length(d.value)] += d.confidence;
            count[length(d.value)] += 1;
        }
        let majority = (1..weight.len()).max_by(|&a, &b| weight[a].total_cmp(&weight[b]))?;
        let mut len = majority;
        // An effect in front of a number hides digits; it rarely adds any. So
        // a longer reading wins over the majority when it keeps repeating
        // exactly. Junk glued to a number's end changes from frame to frame.
        for longer in (len + 1..weight.len()).rev() {
            let needed = (self.observations.len() / 5).max(3);
            if count[longer] < needed {
                continue;
            }
            let Some((candidate, repeats)) = self
                .observations
                .iter()
                .filter(|d| length(d.value) == longer)
                .map(|d| {
                    let same = self.observations.iter().filter(|o| o.value == d.value);
                    (d.value, same.count())
                })
                .max_by_key(|c| c.1)
            else {
                continue;
            };
            // It must also contain what the majority saw: hidden digits leave
            // a fragment of the number, a glued-on neighbor does not.
            let fragment = self
                .observations
                .iter()
                .filter(|d| length(d.value) == majority)
                .map(|d| {
                    let same = self.observations.iter().filter(|o| o.value == d.value);
                    (d.value, same.count())
                })
                .max_by_key(|c| c.1)
                .unwrap_or((0, 0));
            // Two or more extra digits multiply the value a hundredfold. A
            // neighbor's digits glued on can repeat for a few frames, so such
            // a reading must also be seen at least as often as the fragment.
            let trusted = longer == majority + 1 || repeats >= fragment.1;
            if repeats >= 3
                && repeats * 5 >= count[longer] * 3
                && trusted
                && digits_contain(candidate, fragment.0)
            {
                len = longer;
                break;
            }
        }
        // A longer reading was chosen for its consistency, not its share.
        if len == majority && (count[len] < min_count || weight[len] < total * min_share) {
            return None;
        }
        let mut value = 0u32;
        for position in (0..len as u32).rev() {
            let mut digit_weight = [0.0f32; 10];
            let mut digit_count = [0usize; 10];
            for d in self.observations.iter().filter(|d| length(d.value) == len) {
                let digit = (d.value / 10u32.pow(position) % 10) as usize;
                digit_weight[digit] += d.confidence;
                digit_count[digit] += 1;
            }
            let best = (0..10).max_by(|&a, &b| digit_weight[a].total_cmp(&digit_weight[b]))?;
            // No digit may rest on a single reading or a scattered minority.
            if digit_count[best] < min_count.min(2) || digit_weight[best] < weight[len] * 0.40 {
                return None;
            }
            value = value * 10 + best as u32;
        }
        let crit: f32 = self
            .observations
            .iter()
            .filter(|d| length(d.value) == len && d.is_crit)
            .map(|d| d.confidence)
            .sum();
        Some((value, crit > weight[len] * 0.5))
    }

    fn emit(&mut self, value: u32, is_crit: bool) {
        self.emitted = true;
        self.emitted_value = Some(value);
        self.pending = Some(ConfirmedHit {
            value,
            element: self.last.element,
            is_crit,
            x: self.last.x,
            y: self.last.y,
        });
    }

    fn eligible(&mut self, now: Instant) -> bool {
        let age = now.saturating_duration_since(self.created_at);
        if !self.emitted && !self.moved && age > STATIC_LIMIT {
            self.suppressed = true;
        }
        !self.emitted && !self.suppressed && self.moved
    }

    /// Two-digit readings are what sparks and foliage most often look like.
    /// A real one is read in most frames; noise flickers in and out.
    fn plausible(&self, value: u32) -> bool {
        if value >= 100 {
            return true;
        }
        let frames = (self.last_frame - self.first_frame + 1) as f32;
        self.readings >= 8 && self.readings as f32 >= frames * 0.40
    }

    /// Confirm while the number is still visible.
    fn confirm(&mut self, now: Instant) {
        if self.eligible(now) && now.saturating_duration_since(self.created_at) >= CONFIRM_DELAY {
            if let Some((value, crit)) = self.vote(3, 0.50).filter(|v| self.plausible(v.0)) {
                self.emit(value, crit);
            }
        }
    }

    /// Last chance when the number disappears: readings that were too noisy
    /// for a clear majority still count when one value kept recurring.
    fn confirm_on_expiry(&mut self, now: Instant) {
        // In a burst of hits each number is readable only until the next one
        // pops in over it, so the bar is time on screen, not a frame count:
        // capture may run at 20 or at 60 frames a second.
        let seen_for = self.last_seen.saturating_duration_since(self.created_at);
        if self.eligible(now) && seen_for >= MIN_VISIBLE {
            let voted = self.vote(3, 0.40).or_else(|| self.repeated_long_reading());
            if let Some((value, crit)) = voted.filter(|v| self.plausible(v.0)) {
                self.emit(value, crit);
            }
        }
    }

    /// A number of four or more digits that came into view digit by digit and
    /// was then read whole in the last two frames before it vanished. Noise
    /// does not repeat that many digits, and junk glued to a number changes.
    fn repeated_long_reading(&self) -> Option<(u32, bool)> {
        let n = self.observations.len();
        if n < 3 {
            return None;
        }
        let last = self.observations[n - 1];
        (last.value >= 1000 && self.observations[n - 2].value == last.value)
            .then_some((last.value, last.is_crit))
    }

    /// No longer a candidate for new readings.
    fn finished(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last_support) > TRACK_GAP
    }

    fn expired(&self, now: Instant) -> bool {
        // A confirmed track stays as a guard against counting its number again.
        self.finished(now)
            && (!self.emitted
                || now.saturating_duration_since(self.created_at) > NUMBER_LIFETIME * 2)
    }
}

pub struct HitTracker {
    active_tracks: Vec<TrackedHit>,
    last_update: Option<Instant>,
    frame: u64,
    /// Print tracks that end without a confirmed hit (diagnostics only).
    pub trace: bool,
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
            frame: 0,
            trace: false,
        }
    }

    pub fn update(&mut self, detections: &[Detection], scale: f32) -> Vec<ConfirmedHit> {
        self.update_at(detections, scale, Instant::now())
    }

    fn expire(&mut self, now: Instant, out: &mut Vec<ConfirmedHit>) {
        for t in &mut self.active_tracks {
            if !t.finished(now) || t.emitted {
                continue;
            }
            t.confirm_on_expiry(now);
            if self.trace && !t.emitted && t.observations.len() >= 2 {
                let values: Vec<u32> = t.observations.iter().map(|d| d.value).collect();
                eprintln!(
                    "[dropped track] {:?} at ({},{}) moved={} static={} readings={:?}",
                    t.last.element, t.last.x, t.last.y, t.moved, t.suppressed, values
                );
            }
        }
        self.collect(out);
        self.active_tracks.retain(|t| !t.expired(now));
    }

    /// Hand out this frame's confirmations, except a number that was counted
    /// moments ago and then lost from view (hidden by an effect, or swung away
    /// by the camera). Equal hits visible at the same time are all kept.
    fn collect(&mut self, out: &mut Vec<ConfirmedHit>) {
        for i in 0..self.active_tracks.len() {
            let Some(hit) = self.active_tracks[i].pending.take() else {
                continue;
            };
            let track = &self.active_tracks[i];
            let repeat = self.active_tracks.iter().enumerate().any(|(j, other)| {
                j != i
                    && other.pending.is_none()
                    && other.last.element == hit.element
                    && other.emitted_value == Some(hit.value)
                    && other.created_at <= track.created_at
                    && track.created_at.saturating_duration_since(other.created_at)
                        <= NUMBER_LIFETIME
                    && other.last_seen < track.created_at
            });
            // A short-lived misreading of a number that has its own track: the
            // first frames of a pop-in often read only some of its digits.
            let fragment = self.active_tracks.iter().enumerate().any(|(j, other)| {
                if j == i || other.last.element != hit.element || other.suppressed {
                    return false;
                }
                let whole = other
                    .emitted_value
                    .or_else(|| other.vote(1, 0.0).map(|v| v.0))
                    .unwrap_or(other.last.value);
                let apart = if other.created_at > track.created_at {
                    other.created_at.saturating_duration_since(track.created_at)
                } else {
                    track.created_at.saturating_duration_since(other.created_at)
                };
                let (tx, ty) = center(&track.last);
                let near = [&other.first, &other.last].iter().any(|d| {
                    let (ox, oy) = center(d);
                    (tx - ox).abs() <= d.width as f32 && (ty - oy).abs() <= d.height as f32 * 1.5
                });
                digits_contain(whole, hit.value) && apart <= NUMBER_LIFETIME && near
            });
            if self.trace {
                let values: Vec<u32> = track.observations.iter().map(|d| d.value).collect();
                eprintln!(
                    "[confirmed] {} {:?} after {} ms, {} readings {:?}",
                    hit.value,
                    hit.element,
                    track
                        .last_seen
                        .saturating_duration_since(track.created_at)
                        .as_millis(),
                    values.len(),
                    values
                );
            }
            if repeat || fragment {
                if self.trace {
                    let why = if repeat { "repeat" } else { "fragment" };
                    eprintln!("[{why} dropped] {} {:?}", hit.value, hit.element);
                }
                // Not a hit of its own, and no guard for later ones either.
                self.active_tracks[i].emitted_value = None;
            } else {
                out.push(hit);
            }
        }
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
        self.frame += 1;
        let frame = self.frame;
        let scale = if scale.is_finite() {
            scale.max(1.0)
        } else {
            1.0
        };
        let mut confirmed = Vec::new();
        self.expire(now, &mut confirmed);
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

        // Each track's best current reading, for fragment and similarity tests.
        let known: Vec<u32> = self
            .active_tracks
            .iter()
            .map(|t| t.vote(1, 0.0).map_or(t.last.value, |v| v.0))
            .collect();

        // A track that has read one value three times is that number. In a
        // pile of small hits its neighbors sit a few pixels away; a reading
        // that differs by more than one digit is one of them, not a misread,
        // and must not be voted into this track digit by digit.
        let locked: Vec<bool> = self
            .active_tracks
            .iter()
            .zip(&known)
            .map(|(t, &value)| t.observations.iter().filter(|d| d.value == value).count() >= 3)
            .collect();
        let mut used_tracks = vec![false; self.active_tracks.len()];
        let mut used_detections = vec![false; valid.len()];

        // Fragments first. When two numbers overlap, each loses digits to the
        // other; left for the pairing below, the fragment of one would be taken
        // for a new reading of its neighbor and drag that track off its number.
        for (di, d) in valid.iter().enumerate() {
            let (cx, cy) = center(d);
            for (ti, track) in self.active_tracks.iter_mut().enumerate() {
                if track.last.element != d.element || !digits_contain(known[ti], d.value) {
                    continue;
                }
                let (px, py) = track.predicted_center(now);
                let (lx, ly) = center(&track.last);
                let half_w = track.last.width.max(d.width) as f32 * 0.75 + 8.0 * scale;
                let half_h = track.last.height.max(d.height) as f32 * 0.5 + 8.0 * scale;
                let inside = |x: f32, y: f32| (cx - x).abs() <= half_w && (cy - y).abs() <= half_h;
                if inside(px, py) || inside(lx, ly) {
                    // A fragment keeps the track alive without voting.
                    track.last_support = now;
                    used_detections[di] = true;
                    break;
                }
            }
        }

        // Global greedy assignment: a track and detection can each be used only once.
        let mut pairs = Vec::new();
        for (ti, track) in self.active_tracks.iter().enumerate() {
            let since_seen = now.saturating_duration_since(track.last_seen);
            let gap = since_seen.as_secs_f32();
            let (px, py) = track.predicted_center(now);
            for (di, d) in valid.iter().enumerate() {
                if used_detections[di] || d.element != track.last.element {
                    continue;
                }
                // A shorter reading of this number found elsewhere is not
                // evidence for it either.
                if digits_contain(known[ti], d.value) {
                    continue;
                }
                // Only an identical reading earns the wider gates below.
                // Consecutive hits of one attack often differ by a digit or
                // two, so near-equal values are no evidence of identity.
                let same = d.value == track.last.value || d.value == known[ti];
                if !same && locked[ti] && !digits_related(known[ti], d.value) {
                    // Junk glued to this number, or several digits misread,
                    // still sits on the number's own spot. A neighbor is
                    // offset by a good part of the number's size.
                    let (dx, dy) = center(d);
                    let (lx, ly) = center(&track.last);
                    let off_x = (dx - px).abs().min((dx - lx).abs());
                    let off_y = (dy - py).abs().min((dy - ly).abs());
                    if off_x > track.last.width as f32 * 0.6
                        || off_y > track.last.height as f32 * 0.6
                    {
                        continue;
                    }
                }
                // A lingering track only guards against its own number, and
                // only while that number could still be on screen.
                if since_seen > TRACK_GAP
                    && (!same || now.saturating_duration_since(track.created_at) > NUMBER_LIFETIME)
                {
                    continue;
                }
                let hr = d.height as f32 / track.last.height as f32;
                let wr = d.width as f32 / track.last.width as f32;
                // The pop-in animation resizes a number quickly; the value
                // itself is then the stronger identity cue.
                let (h_range, w_range) = if same {
                    (0.30..=3.3, 0.30..=3.3)
                } else {
                    (0.55..=1.8, 0.45..=2.2)
                };
                if !h_range.contains(&hr) || !w_range.contains(&wr) {
                    continue;
                }
                // A number only shrinks after it appears. A different value
                // that is clearly larger is the next hit popping in on top.
                if !same && hr > 1.15 {
                    continue;
                }
                // The camera swings numbers across the screen faster than a
                // velocity estimate can follow, so measure from the last seen
                // position as well as the predicted one.
                let (cx, cy) = center(d);
                let (lx, ly) = center(&track.last);
                let distance = ((cx - px).powi(2) + (cy - py).powi(2))
                    .sqrt()
                    .min(((cx - lx).powi(2) + (cy - ly).powi(2)).sqrt());
                let size = 0.5 * track.last.height.max(d.height) as f32;
                let reach = if same {
                    (60.0 * scale + size + 700.0 * scale * gap).min(450.0 * scale)
                } else {
                    32.0 * scale + size + 220.0 * scale * gap
                };
                if distance > reach {
                    continue;
                }
                let value_penalty = if same { 0.0 } else { 24.0 * scale };
                pairs.push((
                    distance + value_penalty + hr.ln().abs() * 8.0 * scale,
                    ti,
                    di,
                ));
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        for (_, ti, di) in pairs {
            if used_tracks[ti] || used_detections[di] {
                continue;
            }
            used_tracks[ti] = true;
            used_detections[di] = true;
            let track = &mut self.active_tracks[ti];
            track.observe(valid[di], now, scale, frame);
            track.confirm(now);
        }

        // Partial readings: an effect covering some digits yields a shorter
        // number inside a known track, or reveals the rest of a fragment.
        for (di, d) in valid.iter().enumerate() {
            if used_detections[di] {
                continue;
            }
            let (cx, cy) = center(d);
            for (ti, track) in self.active_tracks.iter_mut().enumerate() {
                if track.last.element != d.element {
                    continue;
                }
                let (px, py) = track.predicted_center(now);
                // Generous sideways: a misreading may have shifted the box.
                let half_w = track.last.width.max(d.width) as f32 * 0.75 + 8.0 * scale;
                let half_h = track.last.height.max(d.height) as f32 * 0.5 + 8.0 * scale;
                if (cx - px).abs() > half_w || (cy - py).abs() > half_h {
                    continue;
                }
                let known = known.get(ti).copied().unwrap_or(track.last.value);
                if digits_contain(known, d.value) || digits_contain(track.last.value, d.value) {
                    // A fragment keeps the track alive without voting.
                    track.last_support = now;
                    used_detections[di] = true;
                    break;
                }
                if digits_contain(d.value, known) && !used_tracks[ti] {
                    used_tracks[ti] = true;
                    used_detections[di] = true;
                    track.observe(*d, now, scale, frame);
                    track.confirm(now);
                    break;
                }
            }
        }

        self.collect(&mut confirmed);
        for (di, d) in valid.into_iter().enumerate() {
            if !used_detections[di] {
                self.active_tracks.push(TrackedHit::new(d, now, frame));
            }
        }
        confirmed
    }

    /// A timeout ages tracks but is never a new OCR observation.
    pub fn advance_time(&mut self, now: Instant) -> Vec<ConfirmedHit> {
        if self.last_update.is_some_and(|last| now < last) {
            self.reset();
        }
        self.last_update = Some(now);
        let mut confirmed = Vec::new();
        self.expire(now, &mut confirmed);
        confirmed
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

    fn sized(value: u32, x: i32, y: i32, width: u32) -> Detection {
        Detection {
            width,
            ..detection(value, x, y)
        }
    }

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
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
                at(start, i as u64 * 35),
            ));
        }
        hits.extend(tracker.advance_time(at(start, 5000)));
        assert_eq!(hits.len(), 1);
        hits.extend(tracker.advance_time(at(start, 5000)));
        assert_eq!(hits[0].value, 14229);
    }

    #[test]
    fn numbers_drifting_down_or_sideways_are_confirmed() {
        for (dx, dy) in [(0, 4), (5, 0), (-4, 3)] {
            let mut tracker = HitTracker::new();
            let start = Instant::now();
            let mut hits = Vec::new();
            for i in 0..6 {
                hits.extend(tracker.update_at(
                    &[detection(25364, 900 + dx * i, 400 + dy * i)],
                    1.0,
                    at(start, i as u64 * 33),
                ));
            }
            hits.extend(tracker.advance_time(at(start, 5000)));
            assert_eq!(hits.len(), 1, "drift ({dx},{dy})");
            hits.extend(tracker.advance_time(at(start, 5000)));
            assert_eq!(hits[0].value, 25364);
        }
    }

    #[test]
    fn shrinking_pop_in_counts_as_motion() {
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        let mut hits = Vec::new();
        for (i, height) in [90u32, 60, 40, 30, 30, 30].into_iter().enumerate() {
            let d = Detection {
                height,
                width: height * 3,
                y: 500 - height as i32 / 2,
                ..detection(19191, 800, 0)
            };
            hits.extend(tracker.update_at(&[d], 1.0, at(start, i as u64 * 33)));
        }
        hits.extend(tracker.advance_time(at(start, 5000)));
        assert_eq!(hits.len(), 1);
        hits.extend(tracker.advance_time(at(start, 5000)));
        assert_eq!(hits[0].value, 19191);
    }

    #[test]
    fn fragment_of_a_tracked_number_is_not_a_second_hit() {
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        let mut hits = Vec::new();
        for i in 0..6 {
            hits.extend(tracker.update_at(
                &[sized(19191, 800, 500 + i * 3, 100)],
                1.0,
                at(start, i as u64 * 33),
            ));
        }
        // An effect hides the first three digits for a while.
        for i in 6..13 {
            hits.extend(tracker.update_at(
                &[sized(91, 830, 500 + i * 3, 40)],
                1.0,
                at(start, i as u64 * 33),
            ));
        }
        hits.extend(tracker.advance_time(at(start, 5000)));
        assert_eq!(hits.iter().map(|h| h.value).collect::<Vec<_>>(), [19191]);
    }

    #[test]
    fn fragment_seen_first_is_upgraded_to_the_full_number() {
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        let mut hits = Vec::new();
        for i in 0..2 {
            hits.extend(tracker.update_at(
                &[sized(91, 830, 500 + i * 3, 40)],
                1.0,
                at(start, i as u64 * 33),
            ));
        }
        for i in 2..8 {
            hits.extend(tracker.update_at(
                &[sized(19191, 800, 500 + i * 3, 100)],
                1.0,
                at(start, i as u64 * 33),
            ));
        }
        hits.extend(tracker.advance_time(at(start, 5000)));
        assert_eq!(hits.iter().map(|h| h.value).collect::<Vec<_>>(), [19191]);
    }

    #[test]
    fn number_returning_after_occlusion_is_not_counted_twice() {
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        let mut hits = Vec::new();
        for i in 0..6 {
            hits.extend(tracker.update_at(
                &[detection(6018, 800, 500 + i * 2)],
                1.0,
                at(start, i as u64 * 33),
            ));
        }
        for ms in (200..800).step_by(33) {
            hits.extend(tracker.update_at(&[], 1.0, at(start, ms)));
        }
        for i in 0..6 {
            hits.extend(tracker.update_at(
                &[detection(6018, 805, 520 + i * 2)],
                1.0,
                at(start, 800 + i as u64 * 33),
            ));
        }
        hits.extend(tracker.advance_time(at(start, 5000)));
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn consecutive_hits_with_similar_values_are_both_counted() {
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        let mut hits = Vec::new();
        // 13978 pops in large and settles; 13440 pops in on the same spot.
        let pop = |value: u32, step: i32| Detection {
            height: (150 - step * 20).max(40) as u32,
            width: (560 - step * 70).max(150) as u32,
            ..detection(value, 1000, 600 + step * 2)
        };
        for i in 0..12 {
            hits.extend(tracker.update_at(&[pop(13978, i)], 1.0, at(start, i as u64 * 17)));
        }
        for i in 0..40 {
            hits.extend(tracker.update_at(
                &[pop(13978, 12 + i), pop(13440, i)],
                1.0,
                at(start, 204 + i as u64 * 17),
            ));
        }
        hits.extend(tracker.advance_time(at(start, 5000)));
        hits.sort_by_key(|h| h.value);
        assert_eq!(
            hits.iter().map(|h| h.value).collect::<Vec<_>>(),
            [13440, 13978]
        );
    }

    #[test]
    fn overlapping_numbers_do_not_trade_fragments() {
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        let mut hits = Vec::new();
        // Both drift down with the camera.
        let a =
            |value: u32, step: u64, width: u32| sized(value, 1094, 590 + step as i32 * 3, width);
        let b =
            |value: u32, step: u64, width: u32| sized(value, 1060, 636 + step as i32 * 3, width);
        for i in 0..3 {
            hits.extend(tracker.update_at(
                &[a(104702, i, 180), b(112721, i, 180)],
                1.0,
                at(start, i * 33),
            ));
        }
        // Each hides part of the other for a frame.
        hits.extend(tracker.update_at(&[a(4702, 3, 120), b(12721, 3, 150)], 1.0, at(start, 99)));
        for i in 4..12 {
            hits.extend(tracker.update_at(
                &[a(104702, i, 180), b(112721, i, 180)],
                1.0,
                at(start, i * 33),
            ));
        }
        hits.extend(tracker.advance_time(at(start, 5000)));
        hits.sort_by_key(|h| h.value);
        assert_eq!(
            hits.iter().map(|h| h.value).collect::<Vec<_>>(),
            [104702, 112721]
        );
    }

    #[test]
    fn brief_burst_hit_counts_at_low_frame_rates() {
        // Four readings at 30 fps, then the next number covers it.
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        let mut hits = Vec::new();
        for i in 0..4 {
            hits.extend(tracker.update_at(
                &[detection(1341, 940, 520 + i * 4)],
                1.0,
                at(start, i as u64 * 33),
            ));
        }
        hits.extend(tracker.advance_time(at(start, 5000)));
        assert_eq!(hits.iter().map(|h| h.value).collect::<Vec<_>>(), [1341]);

        // A long number that came into view digit by digit, read whole twice.
        let mut tracker = HitTracker::new();
        let mut hits = Vec::new();
        for (i, value) in [254, 2848, 25480, 25480].into_iter().enumerate() {
            hits.extend(tracker.update_at(
                &[detection(value, 1010, 660 + i as i32 * 4)],
                1.0,
                at(start, i as u64 * 33),
            ));
        }
        hits.extend(tracker.advance_time(at(start, 5000)));
        assert_eq!(hits.iter().map(|h| h.value).collect::<Vec<_>>(), [25480]);

        // Two frames are still too little.
        let mut tracker = HitTracker::new();
        let mut hits = Vec::new();
        for i in 0..2 {
            hits.extend(tracker.update_at(
                &[detection(28727, 1010, 560 + i * 4)],
                1.0,
                at(start, i as u64 * 17),
            ));
        }
        hits.extend(tracker.advance_time(at(start, 5000)));
        assert!(hits.is_empty());
    }

    #[test]
    fn neighbor_in_a_pile_is_not_voted_into_a_known_number() {
        // 351 is read a few times, vanishes under the pile, and a 384 thirty
        // pixels away becomes readable. Two hits, not one "381".
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        let mut hits = Vec::new();
        for i in 0..5 {
            hits.extend(tracker.update_at(
                &[detection(351, 955, 716 - i * 2)],
                1.0,
                at(start, i as u64 * 33),
            ));
        }
        for i in 5..14 {
            hits.extend(tracker.update_at(
                &[detection(384, 909, 680 - i * 2)],
                1.0,
                at(start, i as u64 * 33),
            ));
        }
        hits.extend(tracker.advance_time(at(start, 5000)));
        hits.sort_by_key(|h| h.value);
        assert_eq!(hits.iter().map(|h| h.value).collect::<Vec<_>>(), [351, 384]);
    }

    #[test]
    fn identical_simultaneous_hits_are_counted_separately() {
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        let mut hits = Vec::new();
        for i in 0..6 {
            hits.extend(tracker.update_at(
                &[
                    detection(665, 600, 300 + i * 3),
                    detection(665, 760, 420 + i * 3),
                    detection(665, 900, 150 + i * 3),
                ],
                1.0,
                at(start, i as u64 * 33),
            ));
        }
        hits.extend(tracker.advance_time(at(start, 5000)));
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn timeout_preserves_track_without_adding_observations() {
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        assert!(tracker
            .update_at(&[detection(1000, 800, 500)], 1.0, start)
            .is_empty());
        for ms in [16, 32, 48, 64] {
            assert!(tracker.advance_time(at(start, ms)).is_empty());
        }
        assert!(tracker
            .update_at(&[detection(1000, 800, 497)], 1.0, at(start, 80))
            .is_empty());
        assert!(tracker
            .update_at(&[detection(1000, 800, 494)], 1.0, at(start, 115))
            .is_empty());
        assert_eq!(
            tracker
                .update_at(&[detection(1000, 800, 488)], 1.0, at(start, 500))
                .len(),
            0
        );
        assert_eq!(
            tracker
                .update_at(&[detection(1000, 800, 482)], 1.0, at(start, 620))
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
                .update_at(&[detection(1000, 800, 500)], 1.0, at(start, i * 50))
                .is_empty());
        }
        assert!(tracker
            .advance_time(start + Duration::from_secs(2))
            .is_empty());
        assert!(tracker.active_tracks.is_empty());
    }

    #[test]
    fn long_lived_static_text_stays_rejected_after_it_moves() {
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        for i in 0..40 {
            assert!(tracker
                .update_at(&[detection(1000, 800, 500)], 1.0, at(start, i * 100))
                .is_empty());
        }
        for i in 0..6 {
            assert!(tracker
                .update_at(
                    &[detection(1000, 800, 495 - i * 3)],
                    1.0,
                    at(start, 4000 + i as u64 * 100)
                )
                .is_empty());
        }
    }

    #[test]
    fn nearby_hits_survive_reordered_detections() {
        let mut tracker = HitTracker::new();
        let start = Instant::now();
        let mut hits = Vec::new();
        for i in 0..6 {
            let mut ds = vec![
                detection(1000, 800, 500 - i * 3),
                detection(2000, 820, 500 - i * 3),
            ];
            if i % 2 == 1 {
                ds.reverse();
            }
            hits.extend(tracker.update_at(&ds, 1.0, at(start, i as u64 * 35)));
        }
        hits.extend(tracker.advance_time(at(start, 5000)));
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
        tracker.update_at(&[detection(1000, 800, 497)], 1.0, at(start, 35));
        tracker.reset();
        assert!(tracker
            .update_at(&[detection(1000, 800, 494)], 1.0, at(start, 70))
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
                .update_at(&[d], 1.0, at(start, i as u64 * 35))
                .is_empty());
        }
        assert!(tracker
            .advance_time(start + Duration::from_secs(3))
            .is_empty());
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
                at(start, i as u64 * 35),
            ));
        }
        hits.extend(tracker.advance_time(at(start, 5000)));
        assert_eq!(hits.len(), 1);
        hits.extend(tracker.advance_time(at(start, 5000)));
        assert_eq!(hits[0].value, 42580);
        hits.extend(tracker.advance_time(at(start, 5000)));
        assert_eq!(hits[0].element, ElementType::Pyro);
        assert!(hits[0].is_crit);
    }
}
