use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ElementType {
    Pyro,
    Hydro,
    Cryo,
    Electro,
    Dendro,
    Anemo,
    Geo,
    Physical,
}

impl ElementType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ElementType::Pyro => "pyro",
            ElementType::Hydro => "hydro",
            ElementType::Cryo => "cryo",
            ElementType::Electro => "electro",
            ElementType::Dendro => "dendro",
            ElementType::Anemo => "anemo",
            ElementType::Geo => "geo",
            ElementType::Physical => "physical",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DamageEvent {
    pub id: u64,
    pub timestamp_ms: u64,
    pub value: u32,
    pub element: ElementType,
    pub is_crit: bool,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CombatStats {
    /// Damage per second over the last five seconds.
    pub dps: u32,
    /// Highest five-second DPS reached this session.
    pub peak_dps: u32,
    /// Total damage over the time spent fighting (idle gaps excluded).
    pub avg_dps: u32,
    pub combat_secs: f32,
    pub peak_hit: u32,
    pub peak_element: ElementType,
    pub total_damage: u64,
    pub total_hits: u32,
    pub crit_hits: u32,
    pub crit_rate_pct: f32,
    pub elemental_breakdown: HashMap<ElementType, u64>,
    pub recent_hits: Vec<DamageEvent>,
}

const DPS_WINDOW: Duration = Duration::from_secs(5);
/// Longer than this without a hit ends a fight for the average-DPS clock.
const IDLE_GAP: Duration = Duration::from_secs(5);

pub struct HitRecord {
    pub instant: Instant,
    pub event: DamageEvent,
}

pub struct AppState {
    pub next_id: u64,
    pub hit_history: VecDeque<HitRecord>,
    pub total_damage: u64,
    pub total_hits: u32,
    pub crit_hits: u32,
    pub peak_hit: u32,
    pub peak_element: ElementType,
    pub elemental_breakdown: HashMap<ElementType, u64>,
    pub peak_dps: u32,
    /// Seconds spent in combat: gaps between hits of up to five seconds.
    pub combat_secs: f32,
    pub last_hit: Option<Instant>,
    pub capture_active: bool,
    pub click_through: bool,
    /// Whether screen recorders (and therefore our own capture) see the overlay.
    pub show_in_recordings: bool,
    /// The panel's box inside the overlay window, in physical pixels
    /// (x, y, width, height), as reported by the frontend.
    pub panel_rect: Option<(i32, i32, u32, u32)>,
    /// Invalidates pending vision work across session resets and capture changes.
    pub vision_generation: u64,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            hit_history: VecDeque::with_capacity(500),
            total_damage: 0,
            total_hits: 0,
            crit_hits: 0,
            peak_hit: 0,
            peak_element: ElementType::Physical,
            elemental_breakdown: HashMap::new(),
            peak_dps: 0,
            combat_secs: 0.0,
            last_hit: None,
            capture_active: true,
            click_through: false,
            show_in_recordings: false,
            panel_rect: None,
            vision_generation: 0,
        }
    }

    pub fn record_hit(
        &mut self,
        value: u32,
        element: ElementType,
        is_crit: bool,
        x: i32,
        y: i32,
    ) -> DamageEvent {
        let id = self.next_id;
        self.next_id += 1;

        let now = Instant::now();
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let event = DamageEvent {
            id,
            timestamp_ms,
            value,
            element,
            is_crit,
            x,
            y,
        };

        self.total_damage += value as u64;
        self.total_hits += 1;
        if is_crit {
            self.crit_hits += 1;
        }

        if value > self.peak_hit {
            self.peak_hit = value;
            self.peak_element = element;
        }

        *self.elemental_breakdown.entry(element).or_insert(0) += value as u64;

        self.hit_history.push_back(HitRecord {
            instant: now,
            event: event.clone(),
        });

        // A lone hit, or the first after a pause, counts as one second of combat.
        self.combat_secs += match self.last_hit {
            Some(last) if now.duration_since(last) <= IDLE_GAP => {
                now.duration_since(last).as_secs_f32()
            }
            _ => 1.0,
        };
        self.last_hit = Some(now);
        self.peak_dps = self.peak_dps.max(self.window_dps(now));

        // Prune older than 30 seconds
        let cutoff = now.checked_sub(Duration::from_secs(30)).unwrap_or(now);
        while let Some(front) = self.hit_history.front() {
            if front.instant < cutoff {
                self.hit_history.pop_front();
            } else {
                break;
            }
        }

        event
    }

    fn window_dps(&self, now: Instant) -> u32 {
        let cutoff = now.checked_sub(DPS_WINDOW).unwrap_or(now);
        let damage: u64 = self
            .hit_history
            .iter()
            .rev()
            .take_while(|record| record.instant >= cutoff)
            .map(|record| u64::from(record.event.value))
            .sum();
        (damage as f32 / DPS_WINDOW.as_secs_f32()).round() as u32
    }

    pub fn compute_stats(&self) -> CombatStats {
        let dps = self.window_dps(Instant::now());
        let avg_dps = if self.combat_secs > 0.0 {
            (self.total_damage as f64 / f64::from(self.combat_secs.max(1.0))).round() as u32
        } else {
            0
        };

        let crit_rate_pct = if self.total_hits > 0 {
            (self.crit_hits as f32 / self.total_hits as f32) * 100.0
        } else {
            0.0
        };

        let recent_hits = self
            .hit_history
            .iter()
            .rev()
            .take(15)
            .map(|r| r.event.clone())
            .collect();

        CombatStats {
            dps,
            peak_dps: self.peak_dps.max(dps),
            avg_dps,
            combat_secs: self.combat_secs,
            peak_hit: self.peak_hit,
            peak_element: self.peak_element,
            total_damage: self.total_damage,
            total_hits: self.total_hits,
            crit_hits: self.crit_hits,
            crit_rate_pct,
            elemental_breakdown: self.elemental_breakdown.clone(),
            recent_hits,
        }
    }

    pub fn reset(&mut self) {
        self.vision_generation = self.vision_generation.wrapping_add(1);
        self.hit_history.clear();
        self.total_damage = 0;
        self.total_hits = 0;
        self.crit_hits = 0;
        self.peak_hit = 0;
        self.peak_element = ElementType::Physical;
        self.elemental_breakdown.clear();
        self.peak_dps = 0;
        self.combat_secs = 0.0;
        self.last_hit = None;
    }

    pub fn set_capture_active(&mut self, enabled: bool) {
        if self.capture_active != enabled {
            self.capture_active = enabled;
            self.vision_generation = self.vision_generation.wrapping_add(1);
        }
    }

    pub fn accepts_vision_generation(&self, generation: u64) -> bool {
        self.capture_active && self.vision_generation == generation
    }
}

pub type SharedState = Arc<Mutex<AppState>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_and_pause_invalidate_in_flight_vision_results() {
        let mut state = AppState::new();
        assert!(!state.click_through);
        let generation = state.vision_generation;
        assert!(state.accepts_vision_generation(generation));
        state.reset();
        assert!(!state.accepts_vision_generation(generation));
        let generation = state.vision_generation;
        state.set_capture_active(false);
        state.set_capture_active(true);
        assert!(!state.accepts_vision_generation(generation));
        assert!(state.accepts_vision_generation(state.vision_generation));
    }

    #[test]
    fn test_record_hit_and_stats() {
        let mut state = AppState::new();

        // Record a normal Pyro hit
        state.record_hit(12_000, ElementType::Pyro, false, 500, 400);
        // Record a critical Hydro hit
        state.record_hit(85_000, ElementType::Hydro, true, 520, 380);

        let stats = state.compute_stats();

        assert_eq!(stats.total_hits, 2);
        assert_eq!(stats.crit_hits, 1);
        assert!((stats.crit_rate_pct - 50.0).abs() < 0.1);
        assert_eq!(stats.total_damage, 97_000);
        assert_eq!(stats.peak_hit, 85_000);
        assert_eq!(stats.peak_element, ElementType::Hydro);
        assert_eq!(stats.recent_hits.len(), 2);
        // Two hits moments apart: about one second of combat.
        assert!((1.0..1.5).contains(&stats.combat_secs));
        assert!((64_000..=97_000).contains(&stats.avg_dps));
        assert_eq!(stats.dps, 19_400);
        assert_eq!(stats.peak_dps, 19_400);
        assert_eq!(
            *stats.elemental_breakdown.get(&ElementType::Pyro).unwrap(),
            12_000
        );
        assert_eq!(
            *stats.elemental_breakdown.get(&ElementType::Hydro).unwrap(),
            85_000
        );
        state.reset();
        let stats = state.compute_stats();
        assert_eq!((stats.avg_dps, stats.peak_dps, stats.total_hits), (0, 0, 0));
    }
}
