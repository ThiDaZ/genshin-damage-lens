//! Diagnose segmentation and OCR on an individual screenshot.
use genshin_damage_lens_lib::{
    capture::RawFrame,
    vision::{
        filter::{rgb_to_hsv, ColorFilter},
        DigitMatcher,
    },
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("Usage: inspect_frame <png>")?;
    let image = image::open(path)?.to_rgba8();
    let (width, height) = image.dimensions();
    let mut data = image.into_raw();
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let raw = RawFrame {
        width,
        height,
        stride: width as usize * 4,
        data,
        screen_x: 0,
        screen_y: 0,
    };
    let scale = (height as f32 / 720.0).max(1.0);
    let matcher = DigitMatcher::new();
    for c in ColorFilter::cluster_components_scaled(ColorFilter::segment_frame(&raw), scale) {
        println!(
            "{:?} {:?}: {:?}",
            c.element,
            c.bbox,
            matcher
                .recognize_cluster(&c, scale)
                .map(|h| (h.value, h.confidence))
        );
        for g in c.glyphs {
            let sample = g
                .mask
                .iter()
                .enumerate()
                .find(|(_, v)| **v > 0)
                .map(|(i, _)| {
                    let x = g.bbox.x as usize + i % g.bbox.width as usize;
                    let y = g.bbox.y as usize + i / g.bbox.width as usize;
                    let p = y * raw.stride + x * 4;
                    let rgb = (raw.data[p + 2], raw.data[p + 1], raw.data[p]);
                    (rgb, rgb_to_hsv(rgb.0, rgb.1, rgb.2))
                });
            println!(
                "  {:?}: {:?}, sample {:?}",
                g.bbox,
                matcher.templates.match_glyph(
                    &g.mask,
                    g.bbox.width as usize,
                    g.bbox.height as usize
                ),
                sample
            );
        }
    }
    Ok(())
}
