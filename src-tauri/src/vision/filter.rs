use crate::capture::RawFrame;
use crate::state::ElementType;

#[derive(Debug, Clone, Copy)]
pub struct BoundingBox {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl BoundingBox {
    pub fn intersects(&self, other: &BoundingBox) -> bool {
        self.x < other.x + other.width
            && self.x + self.width > other.x
            && self.y < other.y + other.height
            && self.y + self.height > other.y
    }

    pub fn merge(&self, other: &BoundingBox) -> BoundingBox {
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = (self.x + self.width).max(other.x + other.width);
        let y1 = (self.y + self.height).max(other.y + other.height);
        BoundingBox {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DetectedComponent {
    pub bbox: BoundingBox,
    pub element: ElementType,
    pub pixel_count: usize,
    pub mask: Vec<u8>, // Local binary bitmap of the component
}

#[derive(Debug, Clone)]
pub struct DamageCluster {
    pub element: ElementType,
    pub bbox: BoundingBox,
    pub glyphs: Vec<DetectedComponent>,
    pub is_crit: bool,
}

/// Fast RGB to HSV conversion:
/// Returns (H: 0..=360, S: 0..=100, V: 0..=100)
#[inline(always)]
pub fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (u16, u8, u8) {
    let rf = r as f32;
    let gf = g as f32;
    let bf = b as f32;

    let c_max = rf.max(gf).max(bf);
    let c_min = rf.min(gf).min(bf);
    let delta = c_max - c_min;

    let v = ((c_max / 255.0) * 100.0) as u8;

    if delta <= 0.001 || c_max <= 0.001 {
        return (0, 0, v);
    }

    let s = ((delta / c_max) * 100.0) as u8;

    let mut h = if (rf - c_max).abs() < 0.001 {
        60.0 * (((gf - bf) / delta) % 6.0)
    } else if (gf - c_max).abs() < 0.001 {
        60.0 * (((bf - rf) / delta) + 2.0)
    } else {
        60.0 * (((rf - gf) / delta) + 4.0)
    };

    if h < 0.0 {
        h += 360.0;
    }

    (h.round() as u16, s, v)
}

/// Classify pixel into Genshin damage element
#[inline(always)]
pub fn classify_element(r: u8, g: u8, b: u8) -> Option<ElementType> {
    // Digits in Genshin are brightly lit (V >= 60)
    let (h, s, v) = rgb_to_hsv(r, g, b);

    if v < 60 {
        return None;
    }

    // Physical: High brightness, low saturation (White / Silver)
    if s <= 22 && v >= 82 {
        return Some(ElementType::Physical);
    }

    // Elemental hits must have sufficient saturation
    if s < 35 {
        return None;
    }

    // Element hue boundaries
    match h {
        // Pyro: Red-Orange
        0..=22 | 340..=360 => Some(ElementType::Pyro),
        // Geo: Amber / Gold
        23..=58 => Some(ElementType::Geo),
        // Dendro: Lime / Bright Green
        59..=140 => Some(ElementType::Dendro),
        // Anemo: Turquoise / Mint
        141..=175 => Some(ElementType::Anemo),
        // Cryo: Ice Cyan / Frost Blue
        176..=205 => Some(ElementType::Cryo),
        // Hydro: Deep Ocean Blue
        206..=245 => Some(ElementType::Hydro),
        // Electro: Violet / Magenta
        246..=310 => Some(ElementType::Electro),
        _ => None,
    }
}

pub struct ColorFilter;

impl ColorFilter {
    /// Segments candidate text components from a raw frame
    pub fn segment_frame(frame: &RawFrame) -> Vec<DetectedComponent> {
        let w = frame.width as usize;
        let h = frame.height as usize;
        let stride = frame.stride;
        let data = &frame.data;

        // Skip borders (Genshin damage numbers appear in the central combat area)
        let y_min = (h as f32 * 0.21) as usize; // Exclude top domain banner / header / quest objectives
        let y_max = (h as f32 * 0.85) as usize; // Exclude bottom HP / burst bar
        let x_min = (w as f32 * 0.12) as usize;
        let x_max = (w as f32 * 0.88) as usize;

        // Grid-based sampling for performance (stride 2x2 coarse pass)
        let mut visited = vec![false; w * h];
        let mut components = Vec::new();

        for y in (y_min..y_max).step_by(2) {
            let row_offset = y * stride;
            let yf = y as f32 / h as f32;

            for x in (x_min..x_max).step_by(2) {
                let xf = x as f32 / w as f32;

                // Left quest & item drop exclusion zone (Mora drops, quest text)
                if xf < 0.28 && yf >= 0.22 && yf <= 0.72 {
                    continue;
                }

                // Right party list exclusion zone (characters, portraits, keybinds)
                if xf > 0.82 && yf >= 0.15 && yf <= 0.70 {
                    continue;
                }

                let idx = y * w + x;
                if visited[idx] {
                    continue;
                }

                let px_offset = row_offset + x * 4;
                if px_offset + 3 >= data.len() {
                    continue;
                }

                // BGRA format from DXGI
                let b = data[px_offset];
                let g = data[px_offset + 1];
                let r = data[px_offset + 2];

                if let Some(elem) = classify_element(r, g, b) {
                    // Flood-fill / connected component search within bounding window
                    let comp = Self::extract_component(frame, x, y, elem, &mut visited);
                    if let Some(c) = comp {
                        components.push(c);
                    }
                }
            }
        }

        components
    }

    /// Verify that a candidate component has a dark outline / drop shadow
    /// against its background. Essential for Physical (white) text to reject bright snow/floors/UI.
    fn has_dark_outline(frame: &RawFrame, min_x: usize, max_x: usize, min_y: usize, max_y: usize) -> bool {
        let w = frame.width as usize;
        let h = frame.height as usize;
        let stride = frame.stride;
        let data = &frame.data;

        let mut dark_count = 0usize;
        let mut sample_count = 0usize;

        // Sample 1 pixel outside the bounding box
        let y_top = min_y.saturating_sub(1);
        let y_bot = (max_y + 1).min(h.saturating_sub(1));

        for x in (min_x..=max_x).step_by(2) {
            if x < w {
                for y in [y_top, y_bot] {
                    let off = y * stride + x * 4;
                    if off + 3 < data.len() {
                        let (_, _, v) = rgb_to_hsv(data[off + 2], data[off + 1], data[off]);
                        if v <= 48 {
                            dark_count += 1;
                        }
                        sample_count += 1;
                    }
                }
            }
        }

        let x_left = min_x.saturating_sub(1);
        let x_right = (max_x + 1).min(w.saturating_sub(1));

        for y in (min_y..=max_y).step_by(2) {
            if y < h {
                for x in [x_left, x_right] {
                    let off = y * stride + x * 4;
                    if off + 3 < data.len() {
                        let (_, _, v) = rgb_to_hsv(data[off + 2], data[off + 1], data[off]);
                        if v <= 48 {
                            dark_count += 1;
                        }
                        sample_count += 1;
                    }
                }
            }
        }

        if sample_count == 0 {
            return false;
        }

        (dark_count as f32 / sample_count as f32) >= 0.18
    }

    fn extract_component(
        frame: &RawFrame,
        start_x: usize,
        start_y: usize,
        target_elem: ElementType,
        visited: &mut [bool],
    ) -> Option<DetectedComponent> {
        let w = frame.width as usize;
        let h = frame.height as usize;
        let stride = frame.stride;
        let data = &frame.data;

        let mut queue = Vec::with_capacity(256);
        queue.push((start_x, start_y));
        visited[start_y * w + start_x] = true;

        let mut min_x = start_x;
        let mut max_x = start_x;
        let mut min_y = start_y;
        let mut max_y = start_y;
        let mut count = 0;

        // Max component search radius to avoid exploding on large background planes
        let max_pixels = 3000;

        while let Some((cx, cy)) = queue.pop() {
            count += 1;
            if count > max_pixels {
                return None;
            }

            min_x = min_x.min(cx);
            max_x = max_x.max(cx);
            min_y = min_y.min(cy);
            max_y = max_y.max(cy);

            // 4-neighborhood
            let neighbors = [
                (cx.wrapping_sub(1), cy),
                (cx + 1, cy),
                (cx, cy.wrapping_sub(1)),
                (cx, cy + 1),
            ];

            for (nx, ny) in neighbors {
                if nx < w && ny < h {
                    let n_idx = ny * w + nx;
                    if !visited[n_idx] {
                        let px_offset = ny * stride + nx * 4;
                        if px_offset + 3 < data.len() {
                            let b = data[px_offset];
                            let g = data[px_offset + 1];
                            let r = data[px_offset + 2];

                            if classify_element(r, g, b) == Some(target_elem) {
                                visited[n_idx] = true;
                                queue.push((nx, ny));
                            }
                        }
                    }
                }
            }
        }

        let comp_w = (max_x - min_x + 1) as u32;
        let comp_h = (max_y - min_y + 1) as u32;

        // Exclude components whose vertical position extends into top/bottom UI zones
        if (min_y as f32) < (h as f32 * 0.20) || (max_y as f32) > (h as f32 * 0.85) {
            return None;
        }

        // Height filter for Genshin digit scale: numbers are 15px to max 52px tall at 720p (~70px at 1080p)
        let max_glyph_h = (h as f32 * 0.065).max(52.0) as u32;
        if comp_h < 15 || comp_h > max_glyph_h {
            return None;
        }

        // Aspect ratio filter: single digits or multi-digit clumps
        let aspect = comp_w as f32 / comp_h as f32;
        if aspect < 0.20 || aspect > 4.5 {
            return None;
        }

        // Minimum pixel density (thin digit 1 can have ~14 pixels)
        if count < 14 {
            return None;
        }

        // Physical damage numbers must have a dark outline to reject white snow/floor/dialogues
        if target_elem == ElementType::Physical {
            if !Self::has_dark_outline(frame, min_x, max_x, min_y, max_y) {
                return None;
            }
        }

        // Build binary mask for template matching
        let mut mask = vec![0u8; (comp_w * comp_h) as usize];
        for cy in min_y..=max_y {
            for cx in min_x..=max_x {
                let px_offset = cy * stride + cx * 4;
                if px_offset + 3 < data.len() {
                    let b = data[px_offset];
                    let g = data[px_offset + 1];
                    let r = data[px_offset + 2];
                    if classify_element(r, g, b) == Some(target_elem) {
                        mask[(cy - min_y) * comp_w as usize + (cx - min_x)] = 255;
                    }
                }
            }
        }

        Some(DetectedComponent {
            bbox: BoundingBox {
                x: min_x as u32,
                y: min_y as u32,
                width: comp_w,
                height: comp_h,
            },
            element: target_elem,
            pixel_count: count,
            mask,
        })
    }

    /// Check if a component is an exclamation mark '!' (narrow and has vertical gap before dot)
    pub fn is_exclamation_glyph(c: &DetectedComponent) -> bool {
        let gw = c.bbox.width as usize;
        let gh = c.bbox.height as usize;
        if gw == 0 || gh < 22 {
            return false;
        }
        let aspect = gw as f32 / gh as f32;
        if aspect > 0.40 {
            return false;
        }

        // Must have a gap between the bar and the dot (around 65%..85% of height)
        let bottom_start = (gh as f32 * 0.65) as usize;
        let bottom_end = (gh as f32 * 0.85) as usize;
        for y in bottom_start..bottom_end {
            let mut row_count = 0;
            for x in 0..gw {
                if c.mask[y * gw + x] > 0 {
                    row_count += 1;
                }
            }
            if row_count == 0 {
                return true;
            }
        }
        false
    }

    /// Group individual digit components into horizontally-aligned damage clusters
    pub fn cluster_components(comps: Vec<DetectedComponent>) -> Vec<DamageCluster> {
        let mut sorted = comps;
        sorted.sort_by_key(|c| c.bbox.x);

        let mut raw_clusters: Vec<Vec<DetectedComponent>> = Vec::new();

        for c in sorted {
            // Noise rejection: Genshin damage digits have reasonable height and density
            if c.bbox.height < 15 || c.bbox.height > 52 || c.bbox.width < 5 || c.pixel_count < 14 {
                continue;
            }

            let mut matched_cluster = None;
            for cluster in raw_clusters.iter_mut() {
                let last = cluster.last().unwrap();
                if last.element != c.element {
                    continue;
                }

                // An exclamation mark in Genshin is narrow and has a gap between bar and dot
                let is_last_exclamation = Self::is_exclamation_glyph(last);
                if is_last_exclamation {
                    continue;
                }

                let dy = (last.bbox.y as i32 - c.bbox.y as i32).abs();
                let dh = (last.bbox.height as i32 - c.bbox.height as i32).abs();
                let max_h = last.bbox.height.max(c.bbox.height) as i32;

                let is_curr_exclamation = Self::is_exclamation_glyph(&c);
                let max_allowed_dy = if is_curr_exclamation { (max_h as f32 * 0.50) as i32 } else { (max_h as f32 * 0.35) as i32 };
                let max_allowed_dh = if is_curr_exclamation { (max_h as f32 * 0.55) as i32 } else { (max_h as f32 * 0.35) as i32 };

                if dy <= max_allowed_dy && dh <= max_allowed_dh {
                    let gap = c.bbox.x as i32 - (last.bbox.x + last.bbox.width) as i32;
                    // Horizontal spacing: can touch (-6px) or have gap up to 0.70 * height
                    if gap >= -6 && gap <= (max_h as f32 * 0.70) as i32 {
                        matched_cluster = Some(cluster);
                        break;
                    }
                }
            }

            if let Some(cluster) = matched_cluster {
                cluster.push(c);
            } else {
                raw_clusters.push(vec![c]);
            }
        }

        let mut damage_clusters = Vec::new();
        for glyphs in raw_clusters {
            // Keep clusters with at least 2 glyphs
            if glyphs.len() < 2 {
                continue;
            }

            let element = glyphs[0].element;
            let mut bbox = glyphs[0].bbox;
            let mut is_crit = false;

            for g in &glyphs {
                bbox = bbox.merge(&g.bbox);
                if g.bbox.height >= 28 {
                    is_crit = true;
                }
            }

            damage_clusters.push(DamageCluster {
                element,
                bbox,
                glyphs,
                is_crit,
            });
        }

        damage_clusters
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_element_classification() {
        // Pyro (Vibrant Red-Orange)
        assert_eq!(classify_element(255, 60, 20), Some(ElementType::Pyro));
        // Hydro (Deep Blue)
        assert_eq!(classify_element(30, 140, 255), Some(ElementType::Hydro));
        // Cryo (Light Ice Blue)
        assert_eq!(classify_element(160, 235, 255), Some(ElementType::Cryo));
        // Electro (Purple)
        assert_eq!(classify_element(200, 50, 240), Some(ElementType::Electro));
        // Dendro (Lime Green)
        assert_eq!(classify_element(120, 240, 40), Some(ElementType::Dendro));
        // Geo (Amber/Gold)
        assert_eq!(classify_element(255, 190, 30), Some(ElementType::Geo));
        // Anemo (Mint/Teal)
        assert_eq!(classify_element(40, 240, 190), Some(ElementType::Anemo));
        // Physical (Bright White)
        assert_eq!(classify_element(245, 245, 250), Some(ElementType::Physical));
    }
}
