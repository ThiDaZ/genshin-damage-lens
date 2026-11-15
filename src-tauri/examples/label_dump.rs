//! Draw the segmentation labels of a region of a screenshot, enlarged.
//!
//! label_dump <png> <x,y,w,h> <out.png> [zoom]
//!
//! Lunar-Bloom text is bright green, white (Physical) is white, saturated
//! Dendro text dark green, pale Dendro glow olive, other labels magenta, and
//! unlabeled pixels are the original picture darkened. Boxes of the
//! components that pass the glyph filters are drawn in red.
use genshin_damage_lens_lib::{
    capture::RawFrame,
    vision::filter::{label_pixel, ColorFilter, LUNAR_BLOOM_LABEL},
    vision::DigitMatcher,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        return Err("Usage: label_dump <png> <x,y,w,h> <out.png> [zoom]".into());
    }
    let image = image::open(&args[0])?.to_rgba8();
    let (width, height) = image.dimensions();
    let region: Vec<u32> = args[1].split(',').filter_map(|n| n.parse().ok()).collect();
    let [rx, ry, rw, rh] = region[..] else {
        return Err("region must be x,y,w,h".into());
    };
    let zoom: u32 = args.get(3).and_then(|z| z.parse().ok()).unwrap_or(2);
    let lunar = LUNAR_BLOOM_LABEL;
    let physical = label_pixel(255, 255, 255);
    let dendro_text = label_pixel(0, 202, 80);
    let dendro_glow = label_pixel(150, 214, 184);

    let mut out = image::RgbImage::new(rw * zoom, rh * zoom);
    for y in 0..rh {
        for x in 0..rw {
            let (sx, sy) = (rx + x, ry + y);
            if sx >= width || sy >= height {
                continue;
            }
            let p = image.get_pixel(sx, sy).0;
            let label = label_pixel(p[0], p[1], p[2]);
            let color = if label == lunar {
                [0, 255, 0]
            } else if label == physical {
                [255, 255, 255]
            } else if label == dendro_text {
                [0, 110, 0]
            } else if label == dendro_glow {
                [120, 120, 0]
            } else if label != 0 {
                [200, 0, 200]
            } else {
                [p[0] / 4, p[1] / 4, p[2] / 4]
            };
            for zy in 0..zoom {
                for zx in 0..zoom {
                    out.put_pixel(x * zoom + zx, y * zoom + zy, image::Rgb(color));
                }
            }
        }
    }

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
    for c in ColorFilter::segment_frame(&raw) {
        let b = c.bbox;
        if b.x + b.width <= rx || b.y + b.height <= ry || b.x >= rx + rw || b.y >= ry + rh {
            continue;
        }
        println!(
            "label {} box ({},{} {}x{}) pixels {}",
            c.label, b.x, b.y, b.width, b.height, c.pixel_count
        );
        let color = if c.label == lunar {
            [255, 0, 0]
        } else {
            [255, 140, 0]
        };
        let mut dot = |x: i64, y: i64| {
            let (x, y) = ((x - rx as i64) * zoom as i64, (y - ry as i64) * zoom as i64);
            if x >= 0 && y >= 0 && (x as u32) < rw * zoom && (y as u32) < rh * zoom {
                out.put_pixel(x as u32, y as u32, image::Rgb(color));
            }
        };
        for x in b.x..b.x + b.width {
            dot(x as i64, b.y as i64);
            dot(x as i64, (b.y + b.height) as i64 - 1);
        }
        for y in b.y..b.y + b.height {
            dot(b.x as i64, y as i64);
            dot((b.x + b.width) as i64 - 1, y as i64);
        }
    }
    let scale = (height as f32 / 720.0).max(1.0);
    let matcher = DigitMatcher::new();
    for c in ColorFilter::cluster_components_scaled(ColorFilter::segment_frame(&raw), scale) {
        let b = c.bbox;
        if b.x + b.width <= rx || b.y + b.height <= ry || b.x >= rx + rw || b.y >= ry + rh {
            continue;
        }
        let glyphs: Vec<String> = c
            .glyphs
            .iter()
            .map(|g| {
                let (d, conf) = matcher.templates.match_glyph(
                    &g.mask,
                    g.bbox.width as usize,
                    g.bbox.height as usize,
                );
                format!(
                    "{d}:{conf:.2}@{},{} {}x{}",
                    g.bbox.x, g.bbox.y, g.bbox.width, g.bbox.height
                )
            })
            .collect();
        println!(
            "CLUSTER {:?} ({},{} {}x{}) -> {:?} | {}",
            c.element,
            b.x,
            b.y,
            b.width,
            b.height,
            matcher
                .recognize_cluster(&c, scale)
                .map(|h| (h.value, h.confidence)),
            glyphs.join("  ")
        );
    }
    out.save(&args[2])?;
    Ok(())
}
