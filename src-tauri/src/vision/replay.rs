//! Labeled replay scoring. Event timestamps use recording time, not decode time.
use crate::state::ElementType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LabeledHit {
    pub value: u32,
    pub element: ElementType,
    pub start_ms: u64,
    pub end_ms: u64,
    pub x: i32,
    pub y: i32,
    pub radius: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ObservedHit {
    pub timestamp_ms: u64,
    pub value: u32,
    pub element: ElementType,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReplayReport {
    pub expected: usize,
    pub detected: usize,
    pub matched: usize,
    pub missed: usize,
    pub false_hits: usize,
    /// Unmatched outputs repeating an already matched label's value/time/region.
    pub duplicates: usize,
    pub precision: Option<f64>,
    pub recall: Option<f64>,
    pub expected_damage: u64,
    pub detected_damage: u64,
    pub damage_error: i128,
    pub mean_confirmation_delay_ms: Option<f64>,
    pub missed_labels: Vec<usize>,
    pub unmatched_observations: Vec<usize>,
}

fn distance(label: &LabeledHit, hit: &ObservedHit) -> Option<f64> {
    if label.value != hit.value
        || label.element != hit.element
        || hit.timestamp_ms < label.start_ms
        || hit.timestamp_ms > label.end_ms
    {
        return None;
    }
    let dx = f64::from(hit.x) - f64::from(label.x);
    let dy = f64::from(hit.y) - f64::from(label.y);
    let distance = dx.hypot(dy);
    (distance <= f64::from(label.radius)).then_some(distance)
}

/// Maximum bipartite matching avoids inflating misses when label windows overlap.
pub fn score(expected: &[LabeledHit], observed: &[ObservedHit]) -> Result<ReplayReport, String> {
    if expected
        .iter()
        .any(|h| h.start_ms > h.end_ms || h.radius == 0)
    {
        return Err("Every label needs start_ms <= end_ms and radius > 0".into());
    }
    let edges: Vec<Vec<usize>> = expected
        .iter()
        .map(|label| {
            let mut candidates: Vec<_> = observed
                .iter()
                .enumerate()
                .filter_map(|(i, hit)| distance(label, hit).map(|d| (d, i)))
                .collect();
            candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            candidates.into_iter().map(|(_, i)| i).collect()
        })
        .collect();
    fn assign(
        label: usize,
        edges: &[Vec<usize>],
        seen: &mut [bool],
        owner: &mut [Option<usize>],
    ) -> bool {
        for &observation in &edges[label] {
            if seen[observation] {
                continue;
            }
            seen[observation] = true;
            if owner[observation].is_none()
                || assign(owner[observation].unwrap(), edges, seen, owner)
            {
                owner[observation] = Some(label);
                return true;
            }
        }
        false
    }
    let mut owner = vec![None; observed.len()];
    for label in 0..expected.len() {
        assign(label, &edges, &mut vec![false; observed.len()], &mut owner);
    }
    let mut matched_labels = vec![false; expected.len()];
    let mut delay = 0u128;
    for (i, label) in owner.iter().enumerate() {
        if let Some(label) = label {
            matched_labels[*label] = true;
            delay += u128::from(observed[i].timestamp_ms - expected[*label].start_ms);
        }
    }
    let matched = owner.iter().filter(|x| x.is_some()).count();
    let duplicates = observed
        .iter()
        .enumerate()
        .filter(|(i, hit)| {
            owner[*i].is_none()
                && expected
                    .iter()
                    .enumerate()
                    .any(|(j, label)| matched_labels[j] && distance(label, hit).is_some())
        })
        .count();
    let expected_damage = expected.iter().map(|h| u64::from(h.value)).sum::<u64>();
    let detected_damage = observed.iter().map(|h| u64::from(h.value)).sum::<u64>();
    Ok(ReplayReport {
        expected: expected.len(),
        detected: observed.len(),
        matched,
        missed: expected.len() - matched,
        false_hits: observed.len() - matched,
        duplicates,
        precision: (!observed.is_empty()).then(|| matched as f64 / observed.len() as f64),
        recall: (!expected.is_empty()).then(|| matched as f64 / expected.len() as f64),
        expected_damage,
        detected_damage,
        damage_error: i128::from(detected_damage) - i128::from(expected_damage),
        mean_confirmation_delay_ms: (matched > 0).then(|| delay as f64 / matched as f64),
        missed_labels: matched_labels
            .iter()
            .enumerate()
            .filter_map(|(i, &ok)| (!ok).then_some(i))
            .collect(),
        unmatched_observations: owner
            .iter()
            .enumerate()
            .filter_map(|(i, x)| x.is_none().then_some(i))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn label(value: u32) -> LabeledHit {
        LabeledHit {
            value,
            element: ElementType::Geo,
            start_ms: 0,
            end_ms: 300,
            x: 100,
            y: 100,
            radius: 30,
        }
    }
    fn observed(value: u32) -> ObservedHit {
        ObservedHit {
            value,
            element: ElementType::Geo,
            timestamp_ms: 100,
            x: 100,
            y: 95,
        }
    }
    #[test]
    fn scores_misses_wrong_values_and_duplicates_separately() {
        let report = score(
            &[label(1000), label(2000)],
            &[observed(1000), observed(1000), observed(9000)],
        )
        .unwrap();
        assert_eq!(
            (
                report.matched,
                report.missed,
                report.false_hits,
                report.duplicates
            ),
            (1, 1, 2, 1)
        );
        assert_eq!(report.damage_error, 8000);
        assert_eq!(report.mean_confirmation_delay_ms, Some(100.0));
    }
    #[test]
    fn overlapping_windows_do_not_steal_the_only_match() {
        let broad = label(1000);
        let mut narrow = broad.clone();
        narrow.end_ms = 100;
        let mut late = observed(1000);
        late.timestamp_ms = 200;
        let report = score(&[broad, narrow], &[observed(1000), late]).unwrap();
        assert_eq!(report.matched, 2);
    }
    #[test]
    fn empty_labels_and_invalid_intervals_are_explicit() {
        let report = score(&[], &[]).unwrap();
        assert_eq!((report.precision, report.recall), (None, None));
        let mut bad = label(1000);
        bad.start_ms = 400;
        assert!(score(&[bad], &[]).is_err());
    }
}
