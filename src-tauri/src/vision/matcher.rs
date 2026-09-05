use super::filter::DetectedComponent;

pub struct RecognizedHit {
    pub value: u32,
    pub is_crit: bool,
    pub confidence: f32,
    pub x: i32,
    pub y: i32,
}

const TEMPLATE_W: usize = 16;
const TEMPLATE_H: usize = 24;

/// 16x24 binary glyph templates for digits 0-9 based on Genshin Impact damage font
pub struct DigitTemplates {
    /// 10 digits (0..=9), each 16x24 = 384 bools
    templates: [Vec<u8>; 10],
}

impl DigitTemplates {
    pub fn new() -> Self {
        // Synthesize geometric character masks representing Genshin's bold, rounded damage font
        let mut templates = std::array::from_fn(|_| vec![0u8; TEMPLATE_W * TEMPLATE_H]);

        for (digit, t) in templates.iter_mut().enumerate() {
            Self::rasterize_digit(digit, t);
        }

        Self { templates }
    }

    fn rasterize_digit(digit: usize, buf: &mut [u8]) {
        for y in 0..TEMPLATE_H {
            let yf = y as f32 / (TEMPLATE_H - 1) as f32; // 0.0 to 1.0
            for x in 0..TEMPLATE_W {
                let xf = x as f32 / (TEMPLATE_W - 1) as f32; // 0.0 to 1.0
                let inside = match digit {
                    0 => {
                        let dx = (xf - 0.5) / 0.42;
                        let dy = (yf - 0.5) / 0.45;
                        let dist = dx * dx + dy * dy;
                        dist <= 1.0 && dist >= 0.28
                    }
                    1 => {
                        (xf >= 0.4 && xf <= 0.65 && yf >= 0.1) || (yf <= 0.25 && xf >= 0.25 && xf <= 0.55) || (yf >= 0.85 && xf >= 0.25 && xf <= 0.75)
                    }
                    2 => {
                        (yf <= 0.28 && xf >= 0.2 && xf <= 0.8)
                            || (yf > 0.28 && yf < 0.6 && xf > 0.55)
                            || (yf >= 0.55 && yf <= 0.85 && ((xf - (1.0 - yf)).abs() < 0.2))
                            || (yf >= 0.82 && xf >= 0.15 && xf <= 0.85)
                    }
                    3 => {
                        ((yf <= 0.25 || yf >= 0.75) && xf >= 0.2 && xf <= 0.8)
                            || (xf >= 0.6 && yf >= 0.15 && yf <= 0.85)
                            || (yf >= 0.45 && yf <= 0.55 && xf >= 0.35 && xf <= 0.75)
                    }
                    4 => {
                        (xf >= 0.6 && xf <= 0.82 && yf >= 0.08 && yf <= 0.92)
                            || (yf >= 0.6 && yf <= 0.72 && xf >= 0.15 && xf <= 0.85)
                            || (xf <= 0.4 && yf >= 0.15 && yf <= 0.65)
                    }
                    5 => {
                        (yf <= 0.22 && xf >= 0.15 && xf <= 0.85)
                            || (xf <= 0.38 && yf >= 0.15 && yf <= 0.5)
                            || (yf >= 0.45 && yf <= 0.55 && xf >= 0.2 && xf <= 0.75)
                            || (xf >= 0.62 && yf >= 0.5 && yf <= 0.85)
                            || (yf >= 0.78 && xf >= 0.15 && xf <= 0.8)
                    }
                    6 => {
                        let dx = (xf - 0.5) / 0.4;
                        let dy = (yf - 0.68) / 0.3;
                        let bottom_circle = dx * dx + dy * dy <= 1.0 && dx * dx + dy * dy >= 0.22;
                        let spine = xf <= 0.4 && yf >= 0.15 && yf <= 0.7;
                        let top_arc = yf <= 0.25 && xf >= 0.25 && xf <= 0.75;
                        bottom_circle || spine || top_arc
                    }
                    7 => {
                        (yf <= 0.22 && xf >= 0.15 && xf <= 0.85)
                            || (xf >= 0.55 && yf <= 0.45)
                            || ((xf - (1.0 - yf * 0.7)).abs() < 0.18 && yf > 0.35)
                    }
                    8 => {
                        let dx1 = (xf - 0.5) / 0.38;
                        let dy1 = (yf - 0.3) / 0.26;
                        let top_loop = dx1 * dx1 + dy1 * dy1 <= 1.0 && dx1 * dx1 + dy1 * dy1 >= 0.2;

                        let dx2 = (xf - 0.5) / 0.42;
                        let dy2 = (yf - 0.7) / 0.28;
                        let bot_loop = dx2 * dx2 + dy2 * dy2 <= 1.0 && dx2 * dx2 + dy2 * dy2 >= 0.2;

                        top_loop || bot_loop
                    }
                    9 => {
                        let dx = (xf - 0.5) / 0.4;
                        let dy = (yf - 0.32) / 0.3;
                        let top_circle = dx * dx + dy * dy <= 1.0 && dx * dx + dy * dy >= 0.22;
                        let spine = xf >= 0.6 && yf >= 0.3 && yf <= 0.85;
                        let bot_arc = yf >= 0.75 && xf >= 0.25 && xf <= 0.75;
                        top_circle || spine || bot_arc
                    }
                    _ => false,
                };

                if inside {
                    buf[y * TEMPLATE_W + x] = 255;
                }
            }
        }
    }

    /// Match an arbitrary binary image crop against templates
    pub fn match_glyph(&self, crop: &[u8], cw: usize, ch: usize) -> (u32, f32) {
        if cw == 0 || ch == 0 || crop.is_empty() {
            return (0, 0.0);
        }

        // Resample crop to 16x24
        let mut normalized = [0u8; TEMPLATE_W * TEMPLATE_H];
        for ty in 0..TEMPLATE_H {
            let sy = (ty * ch) / TEMPLATE_H;
            for tx in 0..TEMPLATE_W {
                let sx = (tx * cw) / TEMPLATE_W;
                normalized[ty * TEMPLATE_W + tx] = crop[sy * cw + sx];
            }
        }

        let mut best_digit = 0;
        let mut best_score = -1.0f32;

        for (digit, t) in self.templates.iter().enumerate() {
            let mut match_count = 0usize;
            let mut total_count = 0usize;

            for i in 0..normalized.len() {
                let p1 = normalized[i] > 128;
                let p2 = t[i] > 128;
                if p1 || p2 {
                    total_count += 1;
                    if p1 == p2 {
                        match_count += 1;
                    }
                }
            }

            let score = if total_count > 0 {
                match_count as f32 / total_count as f32
            } else {
                0.0
            };

            if score > best_score {
                best_score = score;
                best_digit = digit as u32;
            }
        }

        (best_digit, best_score)
    }
}

pub struct DigitMatcher {
    templates: DigitTemplates,
}

impl DigitMatcher {
    pub fn new() -> Self {
        Self {
            templates: DigitTemplates::new(),
        }
    }

    /// Segment multi-digit component into individual glyphs and decode integer value
    pub fn recognize(&self, comp: &DetectedComponent) -> Option<RecognizedHit> {
        let w = comp.bbox.width as usize;
        let h = comp.bbox.height as usize;
        let mask = &comp.mask;

        if w < 6 || h < 12 {
            return None;
        }

        // Column projection profile to segment digits
        let mut col_proj = vec![0usize; w];
        for x in 0..w {
            for y in 0..h {
                if mask[y * w + x] > 0 {
                    col_proj[x] += 1;
                }
            }
        }

        // Find glyph segments by column threshold
        let mut segments: Vec<(usize, usize)> = Vec::new();
        let mut in_glyph = false;
        let mut start_x = 0;

        for (x, &count) in col_proj.iter().enumerate() {
            if count > 0 && !in_glyph {
                in_glyph = true;
                start_x = x;
            } else if count == 0 && in_glyph {
                in_glyph = false;
                if x - start_x >= 3 {
                    segments.push((start_x, x - 1));
                }
            }
        }
        if in_glyph && w - start_x >= 3 {
            segments.push((start_x, w - 1));
        }

        if segments.is_empty() {
            return None;
        }

        let mut recognized_digits = Vec::new();
        let mut total_conf = 0.0f32;
        let mut has_crit_mark = false;

        for (gx0, gx1) in segments {
            let gw = gx1 - gx0 + 1;

            // Check if segment is a comma or dot (very small height compared to main font)
            let mut min_y = h;
            let mut max_y = 0;
            for y in 0..h {
                let mut row_has_pixel = false;
                for x in gx0..=gx1 {
                    if mask[y * w + x] > 0 {
                        row_has_pixel = true;
                    }
                }
                if row_has_pixel {
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
            }

            let gh = if max_y >= min_y { max_y - min_y + 1 } else { 0 };

            // Comma filter: bottom-aligned and short
            if gh < h / 3 && min_y > (h * 2) / 3 {
                continue; // Ignore thousands separator comma
            }

            // Exclamation mark or crit burst detector
            if gw <= 5 && gh > (h * 2) / 3 {
                has_crit_mark = true;
            }

            // Extract glyph crop
            let mut glyph_crop = vec![0u8; gw * gh];
            for y in 0..gh {
                for x in 0..gw {
                    glyph_crop[y * gw + x] = mask[(min_y + y) * w + (gx0 + x)];
                }
            }

            let (digit, conf) = self.templates.match_glyph(&glyph_crop, gw, gh);
            if conf >= 0.45 {
                recognized_digits.push(digit);
                total_conf += conf;
            }
        }

        if recognized_digits.is_empty() {
            return None;
        }

        let mut value = 0u32;
        for d in recognized_digits.iter() {
            value = value.saturating_mul(10).saturating_add(*d);
        }

        // Filter out single noise digits or unrealistically large damage
        if value < 10 || value > 99_999_999 {
            return None;
        }

        let avg_conf = total_conf / recognized_digits.len() as f32;

        // In Genshin, critical hits are larger (> 28px height) or accompanied by crit mark
        let is_crit = has_crit_mark || comp.bbox.height >= 28;

        Some(RecognizedHit {
            value,
            is_crit,
            confidence: avg_conf,
            x: comp.bbox.x as i32 + (comp.bbox.width as i32 / 2),
            y: comp.bbox.y as i32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_match_accuracy() {
        let matcher = DigitMatcher::new();

        // Each synthetic template matched against itself should produce high confidence and exact digit
        for digit in 0..10 {
            let template = &matcher.templates.templates[digit];
            let (matched_digit, score) = matcher.templates.match_glyph(template, 16, 24);
            assert_eq!(matched_digit, digit as u32, "Failed to match digit {}", digit);
            assert!(score > 0.95, "Score {} too low for digit {}", score, digit);
        }
    }
}
