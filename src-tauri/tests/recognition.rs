use genshin_damage_lens_lib::{capture::RawFrame, state::ElementType, vision::VisionEngine};

fn load(name: &str) -> RawFrame {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let image = image::open(path)
        .expect("tracked recognition fixture must exist")
        .to_rgba8();
    let (width, height) = image.dimensions();
    let mut data = image.into_raw();
    for p in data.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    RawFrame {
        width,
        height,
        stride: width as usize * 4,
        data,
        screen_x: 0,
        screen_y: 0,
    }
}

#[test]
fn reads_complete_damage_numbers_over_attack_effects() {
    let engine = VisionEngine::new();
    let hits = engine.detect_frame(&load("pyro-over-effects.png"));
    let mut values: Vec<_> = hits.iter().map(|h| (h.value, h.element)).collect();
    values.sort_by_key(|h| h.0);
    assert_eq!(
        values,
        vec![(4046, ElementType::Pyro), (14229, ElementType::Pyro)]
    );
}

#[test]
fn mixed_recording_keeps_physical_hits() {
    let engine = VisionEngine::new();
    let hits = engine.detect_frame(&load("physical-in-geo-sequence.png"));
    assert_eq!(
        hits.len(),
        2,
        "Both visible Physical 261 numbers should be read"
    );
    assert!(hits
        .iter()
        .all(|h| h.value == 261 && h.element == ElementType::Physical));
}

#[test]
fn scenery_does_not_produce_damage() {
    let engine = VisionEngine::new();
    assert!(engine.detect_frame(&load("no-damage.png")).is_empty());
}

#[test]
fn confirms_a_complete_electro_number_once_across_recorded_frames() {
    let mut engine = VisionEngine::new();
    let start = std::time::Instant::now();
    let mut confirmed = Vec::new();
    for ms in [0, 100, 300] {
        let frame = load(&format!("electro-{ms:03}.png"));
        let observations = engine.detect_frame(&frame);
        assert_eq!(observations.len(), 1);
        assert_eq!(
            (observations[0].value, observations[0].element),
            (2066, ElementType::Electro)
        );
        confirmed
            .extend(engine.process_frame_at(&frame, start + std::time::Duration::from_millis(ms)));
    }
    assert_eq!(confirmed.len(), 1);
    assert_eq!(
        (confirmed[0].value, confirmed[0].element),
        (2066, ElementType::Electro)
    );
}
