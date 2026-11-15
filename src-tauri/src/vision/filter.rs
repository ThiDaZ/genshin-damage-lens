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
    /// Segmentation label of the component's pixels (see `label_pixel`). It is
    /// finer than the element: it keeps text apart from effects of the same
    /// element, so components with different labels never belong to one number.
    pub label: u8,
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
    let (h, s, v) = rgb_to_hsv(r, g, b);

    if v < 58 {
        return None;
    }

    // Physical: Strictly neutral white/silver (very low saturation, high brightness)
    if s <= 8 && v >= 85 {
        return Some(ElementType::Physical);
    }
    // White text over the warm stone and sand that fill most arenas: the
    // anti-aliased edge of every stroke is white blended with the floor. With
    // the strict test alone those pixels are dropped, which thins small digits
    // until they no longer match. Warm hues only, and still far paler than Geo
    // (S >= 30) or Pyro (S >= 40) text.
    if s <= 16 && v >= 85 && (10..=54).contains(&h) {
        return Some(ElementType::Physical);
    }

    // Electro: Violet / Magenta / Lavender / Pink
    if (245..=341).contains(&h) && s >= 10 && v >= 60 {
        return Some(ElementType::Electro);
    }

    // Dendro: Leaf Green / Lime / Emerald
    if (55..=155).contains(&h) && s >= 16 && v >= 60 {
        return Some(ElementType::Dendro);
    }

    // Anemo: Turquoise / Mint (text hue is about 160).
    // The border with Cryo (text hue about 180) sits midway between the two.
    // At 175 it cut straight through Cryo glyphs whose edge pixels blend with
    // a warm background, splitting each digit into two elements.
    if (156..=167).contains(&h) && s >= 18 && v >= 60 {
        return Some(ElementType::Anemo);
    }

    // Cryo vs Hydro:
    // Cryo is pale icy cyan with low saturation (S <= 50%).
    // Hydro is saturated ocean blue (S > 50% in 193..=205, or any S >= 18 in 206..=244).
    if (168..=205).contains(&h) && s >= 12 && s <= 50 && v >= 65 {
        return Some(ElementType::Cryo);
    }
    if ((193..=205).contains(&h) && s > 50 && v >= 60)
        || ((206..=244).contains(&h) && s >= 18 && v >= 60)
    {
        return Some(ElementType::Hydro);
    }

    // Pyro vs Geo:
    // Pure red/red-orange: H 0..=22 and 342..=360
    if ((342..=360).contains(&h) || (0..=22).contains(&h)) && s >= 40 && v >= 60 {
        return Some(ElementType::Pyro);
    }
    // Separate saturated orange Pyro from paler amber Geo.
    if (23..=42).contains(&h) {
        let b_ratio = b as f32 / (r as f32).max(1.0);
        if b_ratio <= 0.32 && s >= 45 && v >= 60 {
            return Some(ElementType::Pyro);
        } else if b_ratio >= 0.35 && s >= 30 && v >= 60 {
            return Some(ElementType::Geo);
        }
    }
    if (43..=54).contains(&h) && s >= 30 && v >= 60 {
        return Some(ElementType::Geo);
    }

    None
}

/// Dendro text is fully saturated green (about RGB 0,202,80). The glow that
/// Lunar-Bloom, Lauma's field and other Dendro effects lay over the fight is the
/// same hue but pale (saturation 20-45 %). Anti-aliased text edges fall in
/// between; 60 % keeps the glyph body and leaves the glow out.
const DENDRO_TEXT_SATURATION: u8 = 60;

const ELEMENTS: [ElementType; 8] = [
    ElementType::Pyro,
    ElementType::Hydro,
    ElementType::Cryo,
    ElementType::Electro,
    ElementType::Dendro,
    ElementType::Anemo,
    ElementType::Geo,
    ElementType::Physical,
];

/// Label of Lunar-Bloom damage text, one past the labels of `ELEMENTS`.
pub const LUNAR_BLOOM_LABEL: u8 = 1 + ((ELEMENTS.len() as u8) << 1);

/// Tallest glyph accepted, in pixels at 720p. A number pops in several times
/// its final size and shrinks within a few frames. For most text the frames
/// after that are enough. Lunar-Bloom hits land as two or three equal numbers
/// on one spot, where each is whole only while it is still enlarged on top of
/// the others, so those are read at up to about 3.5 times their final size.
fn max_glyph_height(label: u8) -> f32 {
    if label == LUNAR_BLOOM_LABEL {
        150.0
    } else {
        110.0
    }
}

/// Lean of Lunar-Bloom digits: pixels to the right per pixel up.
const LUNAR_SLANT: f32 = 0.27;

/// Lunar-Bloom numbers are not drawn like other damage text. Each digit is a
/// gradient at full brightness, from pale green at the top (about RGB
/// 185,253,180: saturation 30 %) to almost white at the bottom (250,253,239:
/// saturation 5 %), inside a dark green outline, and the digits lean right.
/// The element classes cut such a digit into Dendro, nothing and Physical, so
/// the green part of the gradient gets a label of its own. Saturation of 8 %
/// and less stays Physical: white text over green scenery is tinted that much,
/// and taking those pixels broke up white numbers. The white end of a digit is
/// attached again in `lunar_glyph_mask`.
#[inline(always)]
fn is_lunar_bloom_text(r: u8, g: u8, b: u8) -> bool {
    let (h, s, v) = rgb_to_hsv(r, g, b);
    v >= 88 && (60..=150).contains(&h) && (9..=45).contains(&s)
}

/// Per-pixel segmentation label: 0 for background, otherwise the element plus
/// one bit that splits Pyro (orange text vs. red effects), Electro (purple
/// text vs. reddish scenery) and Dendro (saturated text vs. pale glow), or
/// `LUNAR_BLOOM_LABEL`. Two pixels belong together iff labels are equal.
#[inline(always)]
pub fn label_pixel(r: u8, g: u8, b: u8) -> u8 {
    // Same cut as `v < 58` in classify_element, without the float work.
    if r.max(g).max(b) <= 147 {
        return 0;
    }
    if is_lunar_bloom_text(r, g, b) {
        return LUNAR_BLOOM_LABEL;
    }
    let Some(element) = classify_element(r, g, b) else {
        return 0;
    };
    let split = match element {
        ElementType::Pyro => (23..=42).contains(&rgb_to_hsv(r, g, b).0),
        ElementType::Electro => rgb_to_hsv(r, g, b).0 <= 305,
        ElementType::Dendro => rgb_to_hsv(r, g, b).1 >= DENDRO_TEXT_SATURATION,
        _ => false,
    };
    let index = ELEMENTS.iter().position(|&e| e == element).unwrap_or(0) as u8;
    1 + (index << 1 | u8::from(split))
}

fn element_of_label(label: u8) -> Option<ElementType> {
    if label == LUNAR_BLOOM_LABEL {
        // Lunar-Bloom damage is Dendro damage.
        return Some(ElementType::Dendro);
    }
    ELEMENTS.get((label.checked_sub(1)? >> 1) as usize).copied()
}

/// Cut one Lunar-Bloom glyph out of the frame.
///
/// The lower third of a digit fades to white, which is Physical by color, so
/// the labeled component is only the upper part. Starting from that part,
/// collect the Lunar-Bloom and white pixels connected to it, inside a window
/// that reaches below the component by 0.7 of its height (and left by the
/// lean that goes with it). Connectivity also keeps leaning neighbors, whose boxes
/// overlap this one, out of the mask. Returns the box origin, size and mask.
fn lunar_glyph_mask(
    labels: &[u8],
    w: usize,
    h: usize,
    first: &Run,
    extent: &Extent,
) -> (usize, usize, usize, usize, Vec<u8>) {
    let physical = 1 + ((ELEMENTS.len() as u8 - 1) << 1);
    debug_assert_eq!(element_of_label(physical), Some(ElementType::Physical));
    let comp_h = (extent.max_y - extent.min_y + 1) as usize;
    let below = (comp_h * 7).div_ceil(10);
    let x0 = (extent.min_x as usize).saturating_sub((LUNAR_SLANT * below as f32) as usize + 3);
    let x1 = (extent.max_x as usize + 3).min(w - 1);
    let y0 = extent.min_y as usize;
    let y1 = (extent.max_y as usize + below).min(h - 1);
    let (ww, wh) = (x1 - x0 + 1, y1 - y0 + 1);

    let mut seen = vec![0u8; ww * wh];
    let start = (first.y as usize - y0) * ww + first.x0 as usize - x0;
    seen[start] = 255;
    let mut stack = vec![start];
    let (mut lo_x, mut hi_x, mut hi_y) = (ww, 0, 0);
    while let Some(p) = stack.pop() {
        let (x, y) = (p % ww, p / ww);
        lo_x = lo_x.min(x);
        hi_x = hi_x.max(x);
        hi_y = hi_y.max(y);
        let mut visit = |nx: usize, ny: usize| {
            let label = labels[(y0 + ny) * w + x0 + nx];
            let q = ny * ww + nx;
            if seen[q] == 0 && (label == LUNAR_BLOOM_LABEL || label == physical) {
                seen[q] = 255;
                stack.push(q);
            }
        };
        if x > 0 {
            visit(x - 1, y);
        }
        if x + 1 < ww {
            visit(x + 1, y);
        }
        if y > 0 {
            visit(x, y - 1);
        }
        if y + 1 < wh {
            visit(x, y + 1);
        }
    }

    let (out_w, out_h) = (hi_x - lo_x + 1, hi_y + 1);
    let mut mask = vec![0u8; out_w * out_h];
    for (y, out) in mask.chunks_exact_mut(out_w).enumerate() {
        out.copy_from_slice(&seen[y * ww + lo_x..=y * ww + hi_x]);
    }
    (x0 + lo_x, y0, out_w, out_h, mask)
}

/// Straighten a right-leaning glyph mask: every row moves right in proportion
/// to its distance from the top, then empty columns are cut off. Returns the
/// mask, its width, and where its left edge lies relative to the old one when
/// the bottom row stays in place.
fn deskew(mask: &[u8], w: usize, h: usize, slant: f32) -> (Vec<u8>, usize, i64) {
    let shift = |y: usize| (slant * y as f32).round() as usize;
    let wide = w + shift(h - 1);
    let (mut lo, mut hi) = (wide, 0);
    for (y, row) in mask.chunks_exact(w).enumerate() {
        for (x, _) in row.iter().enumerate().filter(|(_, &m)| m > 0) {
            lo = lo.min(x + shift(y));
            hi = hi.max(x + shift(y));
        }
    }
    if lo > hi {
        return (mask.to_vec(), w, 0);
    }
    let out_w = hi - lo + 1;
    let mut out = vec![0u8; out_w * h];
    for (y, row) in mask.chunks_exact(w).enumerate() {
        for (x, _) in row.iter().enumerate().filter(|(_, &m)| m > 0) {
            out[y * out_w + x + shift(y) - lo] = 255;
        }
    }
    (out, out_w, lo as i64 - shift(h - 1) as i64)
}

/// Threads used to label a frame. Four at most, and never more than half the
/// logical cores: next to a game and a recorder that keep every core busy,
/// more threads only wait for each other. Measured with all cores loaded,
/// eight workers took about twice as long per frame as four. `GDL_WORKERS`
/// overrides the choice for experiments.
fn label_workers() -> usize {
    static WORKERS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *WORKERS.get_or_init(|| {
        std::env::var("GDL_WORKERS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&n| n >= 1)
            .unwrap_or_else(|| {
                let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
                (cores / 2).clamp(1, 4)
            })
    })
}

/// A maximal horizontal stretch of equally labeled pixels.
struct Run {
    x0: u32,
    /// Exclusive end.
    x1: u32,
    y: u32,
    label: u8,
    /// Touches the coarse scan grid inside the combat viewport.
    seeded: bool,
}

/// Classify every pixel once, across cores, and collect the row runs that
/// connected-component labeling works on. Everything downstream compares
/// bytes or whole runs instead of re-deriving colors per pixel.
fn label_frame(frame: &RawFrame, zones: &HudZones) -> (Vec<u8>, Vec<Run>) {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let stride = frame.stride;
    let data = &frame.data;
    let table = label_table();

    // Dynamic combat viewport boundaries
    let y_min = (h as f32 * 0.08) as usize; // Exclude top status/header
    let y_max = (h as f32 * 0.90) as usize; // Exclude bottom player burst bar
    let x_min = (w as f32 * 0.06) as usize;
    let x_max = (w as f32 * 0.94) as usize;

    // The skill zone is everything right of one column.
    let skill_from = zones
        .columns
        .iter()
        .position(|c| c & SKILL_ZONE != 0)
        .unwrap_or(w);

    let mut labels = vec![0u8; w * h];
    let workers = label_workers();
    // Rows far above or below the combat viewport cannot hold an accepted
    // component: anything reaching past the 8 % / 91 % lines is rejected. One
    // row beyond each line is kept so such a component is still seen crossing.
    let row_lo = ((h as f32 * 0.08).ceil() as usize).saturating_sub(1);
    let row_hi = ((h as f32 * 0.91) as usize + 2).min(h);
    let band = (row_hi - row_lo).div_ceil(workers).max(1);
    let mut runs = Vec::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = labels[row_lo * w..row_hi * w]
            .chunks_mut(band * w)
            .enumerate()
            .map(|(bi, chunk)| {
                scope.spawn(move || {
                    let mut runs: Vec<Run> = Vec::with_capacity(4096);
                    for (ry, row) in chunk.chunks_exact_mut(w).enumerate() {
                        let y = row_lo + bi * band + ry;
                        let start = y * stride;
                        let pixels = &data[start..start + w * 4];
                        let zy = zones.rows[y];
                        let skill_row = zy & SKILL_ZONE != 0;
                        // The coarse scan only looks at even rows and columns.
                        let scan_row = y % 2 == 0 && (y_min..y_max).contains(&y);
                        // Two tight passes: classify the row, then cut it
                        // into runs. Mixing them kept the compiler from
                        // producing a simple loop for either.
                        for (label, px) in row.iter_mut().zip(pixels.chunks_exact(4)) {
                            // BGRA format from DXGI
                            *label = table.label(px[2], px[1], px[0]);
                        }
                        // Skill icons and cooldown digits are a wall.
                        if skill_row {
                            row[skill_from..].fill(0);
                        }
                        let mut x = 0;
                        while x < w {
                            // Background comes in long stretches: skip it
                            // eight pixels at a time.
                            if let Some(word) = row.get(x..x + 8) {
                                if word == [0u8; 8] {
                                    x += 8;
                                    continue;
                                }
                            }
                            let value = row[x];
                            if value == 0 {
                                x += 1;
                                continue;
                            }
                            let x0 = x;
                            while x < w && row[x] == value {
                                x += 1;
                            }
                            runs.push(Run {
                                x0: x0 as u32,
                                x1: x as u32,
                                y: y as u32,
                                label: value,
                                seeded: false,
                            });
                        }
                        if scan_row {
                            for run in runs.iter_mut().rev().take_while(|r| r.y == y as u32) {
                                let from = (run.x0 as usize).max(x_min).next_multiple_of(2);
                                let to = (run.x1 as usize).min(x_max);
                                run.seeded =
                                    (from..to).step_by(2).any(|x| zones.columns[x] & zy == 0);
                            }
                        }
                    }
                    runs
                })
            })
            .collect();
        for handle in handles {
            runs.extend(handle.join().expect("label worker panicked"));
        }
    });
    (labels, runs)
}

/// Exact label for every 24-bit color, built once, in two levels.
///
/// Colors are grouped into 16×16×16 blocks. Most blocks lie wholly inside one
/// element's color range or wholly outside all of them, and are answered from
/// a 4096-entry table that stays in the first-level cache. Only blocks that
/// straddle a boundary keep their 4096 individual labels. A flat 16 MB table
/// gave the same answers but missed the cache on most pixels.
struct LabelTable {
    /// Below `MIXED`: the label of a uniform block. Otherwise `MIXED` plus the
    /// index of the block's own labels in `mixed`.
    blocks: Box<[u16; 4096]>,
    mixed: Vec<u8>,
}

const MIXED: u16 = 256;

impl LabelTable {
    #[inline(always)]
    fn label(&self, r: u8, g: u8, b: u8) -> u8 {
        // Three 4-bit fields: the index is below 4096 by construction, which
        // lets the fixed-size table be read without a bounds check.
        let block =
            self.blocks[usize::from(r >> 4) << 8 | usize::from(g >> 4) << 4 | usize::from(b >> 4)];
        if block < MIXED {
            block as u8
        } else {
            self.mixed[usize::from(block - MIXED) << 12
                | usize::from(r & 15) << 8
                | usize::from(g & 15) << 4
                | usize::from(b & 15)]
        }
    }

    fn build() -> Self {
        let block_labels = |block: usize| -> Vec<u8> {
            let (r, g, b) = (block >> 8 << 4, (block >> 4 & 15) << 4, (block & 15) << 4);
            (0..4096usize)
                .map(|i| {
                    label_pixel(
                        (r | i >> 8) as u8,
                        (g | (i >> 4 & 15)) as u8,
                        (b | (i & 15)) as u8,
                    )
                })
                .collect()
        };
        let workers = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .clamp(1, 16);
        let per_worker = 4096usize.div_ceil(workers);
        let mut parts: Vec<Vec<Vec<u8>>> = Vec::new();
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|wi| {
                    scope.spawn(move || {
                        (wi * per_worker..((wi + 1) * per_worker).min(4096))
                            .map(block_labels)
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            for handle in handles {
                parts.push(handle.join().expect("label table worker panicked"));
            }
        });
        let mut blocks = Box::new([0u16; 4096]);
        let mut mixed = Vec::new();
        for (block, labels) in blocks.iter_mut().zip(parts.into_iter().flatten()) {
            if labels.iter().all(|&l| l == labels[0]) {
                *block = u16::from(labels[0]);
            } else {
                *block = MIXED + (mixed.len() >> 12) as u16;
                mixed.extend_from_slice(&labels);
            }
        }
        Self { blocks, mixed }
    }
}

fn label_table() -> &'static LabelTable {
    static TABLE: std::sync::OnceLock<LabelTable> = std::sync::OnceLock::new();
    TABLE.get_or_init(LabelTable::build)
}

#[derive(Clone, Copy)]
struct Extent {
    min_x: u32,
    max_x: u32,
    min_y: u32,
    max_y: u32,
    count: usize,
    seeded: bool,
}

const SKILL_ZONE: u8 = 8;

/// HUD areas that never contain damage numbers, as per-column and per-row bit
/// flags. A pixel is excluded when its column and row share a flag.
struct HudZones {
    columns: Vec<u8>,
    rows: Vec<u8>,
}

impl HudZones {
    fn new(w: usize, h: usize) -> Self {
        let columns = (0..w)
            .map(|x| {
                let xf = x as f32 / w as f32;
                u8::from(xf < 0.16) // minimap & quest HUD (top-left)
                    | u8::from(xf > 0.35 && xf < 0.65) << 1 // player HP bar
                    | u8::from(xf > 0.89) << 2 // party member icons
                    | u8::from(xf > 0.83) << 3 // skill and burst icons
            })
            .collect();
        let rows = (0..h)
            .map(|y| {
                let yf = y as f32 / h as f32;
                u8::from(yf < 0.28)
                    | u8::from(yf > 0.88) << 1
                    | u8::from((0.15..=0.65).contains(&yf)) << 2
                    | u8::from(yf > 0.66) << 3
            })
            .collect();
        Self { columns, rows }
    }
}

pub struct ColorFilter;

impl ColorFilter {
    /// Segments candidate text components from a raw frame
    pub fn segment_frame(frame: &RawFrame) -> Vec<DetectedComponent> {
        let w = frame.width as usize;
        let h = frame.height as usize;
        let stride = frame.stride;
        if w == 0 || h == 0 || stride < w * 4 || frame.data.len() < (h - 1) * stride + w * 4 {
            return Vec::new();
        }

        let scale = (h as f32 / 720.0).max(1.0);
        let zones = HudZones::new(w, h);
        let (labels, runs) = label_frame(frame, &zones);

        // Connected components over runs: union overlapping same-label runs of
        // adjacent rows (4-connectivity). The smaller index stays the root, so
        // a root is its component's top-left run.
        let mut row_start = vec![runs.len() as u32; h + 1];
        for (i, run) in runs.iter().enumerate().rev() {
            row_start[run.y as usize] = i as u32;
        }
        for y in (0..h).rev() {
            row_start[y] = row_start[y].min(row_start[y + 1]);
        }
        let mut parent: Vec<u32> = (0..runs.len() as u32).collect();
        fn find(parent: &mut [u32], mut i: u32) -> u32 {
            while parent[i as usize] != i {
                parent[i as usize] = parent[parent[i as usize] as usize];
                i = parent[i as usize];
            }
            i
        }
        for y in 1..h {
            let (mut a, a_end) = (row_start[y - 1], row_start[y]);
            let (mut b, b_end) = (row_start[y], row_start[y + 1]);
            while a < a_end && b < b_end {
                let (above, below) = (&runs[a as usize], &runs[b as usize]);
                if above.label == below.label && above.x0 < below.x1 && below.x0 < above.x1 {
                    let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                    if ra != rb {
                        parent[ra.max(rb) as usize] = ra.min(rb);
                    }
                }
                if above.x1 <= below.x1 {
                    a += 1;
                } else {
                    b += 1;
                }
            }
        }

        let mut extents: Vec<Option<Extent>> = vec![None; runs.len()];
        for (i, run) in runs.iter().enumerate() {
            let root = find(&mut parent, i as u32) as usize;
            let e = extents[root].get_or_insert(Extent {
                min_x: run.x0,
                max_x: run.x1 - 1,
                min_y: run.y,
                max_y: run.y,
                count: 0,
                seeded: false,
            });
            e.min_x = e.min_x.min(run.x0);
            e.max_x = e.max_x.max(run.x1 - 1);
            e.max_y = run.y;
            e.count += (run.x1 - run.x0) as usize;
            e.seeded |= run.seeded;
        }

        extents
            .iter()
            .enumerate()
            .filter_map(|(root, extent)| {
                let extent = extent.as_ref().filter(|e| e.seeded)?;
                Self::build_component(frame, &labels, &runs[root], extent, scale)
            })
            .collect()
    }

    /// Calculate ratio of dark pixels immediately surrounding a bounding box
    pub fn dark_outline_ratio(
        frame: &RawFrame,
        min_x: usize,
        max_x: usize,
        min_y: usize,
        max_y: usize,
    ) -> f32 {
        let w = frame.width as usize;
        let h = frame.height as usize;
        let stride = frame.stride;
        let data = &frame.data;

        let scale = (h as f32 / 720.0).max(1.0);
        let dist = ((1.0 * scale).round() as usize).max(1);

        let mut dark_count = 0usize;
        let mut sample_count = 0usize;

        // Sample outside the bounding box
        let y_top = min_y.saturating_sub(dist);
        let y_bot = (max_y + dist).min(h.saturating_sub(1));

        for x in (min_x..=max_x).step_by(2) {
            if x < w {
                for y in [y_top, y_bot] {
                    let off = y * stride + x * 4;
                    if off + 3 < data.len() {
                        let (_, _, v) = rgb_to_hsv(data[off + 2], data[off + 1], data[off]);
                        if v <= 50 {
                            dark_count += 1;
                        }
                        sample_count += 1;
                    }
                }
            }
        }

        let x_left = min_x.saturating_sub(dist);
        let x_right = (max_x + dist).min(w.saturating_sub(1));

        for y in (min_y..=max_y).step_by(2) {
            if y < h {
                for x in [x_left, x_right] {
                    let off = y * stride + x * 4;
                    if off + 3 < data.len() {
                        let (_, _, v) = rgb_to_hsv(data[off + 2], data[off + 1], data[off]);
                        if v <= 50 {
                            dark_count += 1;
                        }
                        sample_count += 1;
                    }
                }
            }
        }

        if sample_count == 0 {
            return 0.0;
        }

        dark_count as f32 / sample_count as f32
    }

    /// Verify that a candidate component has a dark outline / drop shadow
    pub fn has_dark_outline(
        frame: &RawFrame,
        min_x: usize,
        max_x: usize,
        min_y: usize,
        max_y: usize,
    ) -> bool {
        Self::dark_outline_ratio(frame, min_x, max_x, min_y, max_y) >= 0.18
    }

    /// Apply the glyph size filters to one connected component and cut its mask.
    fn build_component(
        frame: &RawFrame,
        labels: &[u8],
        first: &Run,
        extent: &Extent,
        scale: f32,
    ) -> Option<DetectedComponent> {
        let w = frame.width as usize;
        let h = frame.height as usize;

        // Damage text can share its element with an enormous attack effect.
        // The label also separates orange text from red effects within Pyro,
        // purple text from reddish scenery within Electro and saturated text
        // from pale glow within Dendro.
        let seed = first.label;
        let target_elem = element_of_label(seed)?;

        // Large background planes and effects are not text.
        // Text grows briefly on impact; area scales in two dimensions.
        let count = extent.count;
        if count > (12_000.0 * scale * scale) as usize {
            return None;
        }
        let (min_x, max_x) = (extent.min_x as usize, extent.max_x as usize);
        let (min_y, max_y) = (extent.min_y as usize, extent.max_y as usize);

        let comp_w = (max_x - min_x + 1) as u32;
        let comp_h = (max_y - min_y + 1) as u32;

        // Exclude components extending into extreme top/bottom margins
        if (min_y as f32) < (h as f32 * 0.08) || (max_y as f32) > (h as f32 * 0.91) {
            return None;
        }

        // Height filter scaled by viewport resolution: supports 720p through 4K
        let min_glyph_h = (12.0 * scale) as u32;
        let max_glyph_h = (max_glyph_height(seed) * scale) as u32;
        if comp_h < min_glyph_h || comp_h > max_glyph_h {
            return None;
        }

        // Aspect ratio filter: single digits to wide multi-digit clumps
        let aspect = comp_w as f32 / comp_h as f32;
        if aspect < 0.18 || aspect > 8.5 {
            return None;
        }

        // Minimum pixel density
        if count < (10.0 * scale) as usize {
            return None;
        }

        // Build binary mask for template matching
        let mut mask = vec![0u8; (comp_w * comp_h) as usize];
        for (cy, out) in (min_y..=max_y).zip(mask.chunks_exact_mut(comp_w as usize)) {
            let row = &labels[cy * w + min_x..=cy * w + max_x];
            for (m, &label) in out.iter_mut().zip(row) {
                if label == seed {
                    *m = 255;
                }
            }
        }

        // A gray outline over a bright attack effect may never reach the absolute
        // dark threshold. Also measure contrast along the actual glyph boundary.
        if target_elem == ElementType::Physical
            && !Self::has_dark_outline(frame, min_x, max_x, min_y, max_y)
            && !Self::has_contrasting_stroke(
                frame,
                min_x,
                min_y,
                comp_w as usize,
                comp_h as usize,
                &mask,
            )
        {
            return None;
        }

        // Lunar-Bloom digits are completed by their white lower part and
        // straightened: everything downstream expects whole, upright digits.
        if seed == LUNAR_BLOOM_LABEL {
            let (x, y, gw, gh, mask) = lunar_glyph_mask(labels, w, h, first, extent);
            let pixel_count = mask.iter().filter(|&&m| m > 0).count();
            let (mask, gw, dx) = deskew(&mask, gw, gh, LUNAR_SLANT);
            return Some(DetectedComponent {
                bbox: BoundingBox {
                    x: (x as i64 + dx).max(0) as u32,
                    y: y as u32,
                    width: gw as u32,
                    height: gh as u32,
                },
                element: target_elem,
                label: seed,
                pixel_count,
                mask,
            });
        }

        Some(DetectedComponent {
            bbox: BoundingBox {
                x: min_x as u32,
                y: min_y as u32,
                width: comp_w,
                height: comp_h,
            },
            element: target_elem,
            label: seed,
            pixel_count: count,
            mask,
        })
    }

    fn has_contrasting_stroke(
        frame: &RawFrame,
        x0: usize,
        y0: usize,
        w: usize,
        h: usize,
        mask: &[u8],
    ) -> bool {
        let distance = (frame.height as f32 / 720.0).round().max(1.0) as i64;
        let inside = |x: i64, y: i64| {
            x >= 0
                && y >= 0
                && x < w as i64
                && y < h as i64
                && mask[y as usize * w + x as usize] > 0
        };
        let brightness = |x: usize, y: usize| {
            let p = y * frame.stride + x * 4;
            // Perceived brightness, not max(R,G,B): saturated blue effects can
            // have a 255 channel while still strongly contrasting with white text.
            ((54 * u32::from(frame.data[p + 2])
                + 183 * u32::from(frame.data[p + 1])
                + 19 * u32::from(frame.data[p]))
                / 256) as u8
        };
        let mut boundary = 0;
        let mut contrasted = 0;
        for y in 0..h {
            for x in 0..w {
                if !inside(x as i64, y as i64) {
                    continue;
                }
                let directions = [(-1, 0), (1, 0), (0, -1), (0, 1)];
                if directions
                    .iter()
                    .all(|&(dx, dy)| inside(x as i64 + dx, y as i64 + dy))
                {
                    continue;
                }
                let value = brightness(x0 + x, y0 + y);
                let mut contrast = false;
                for (dx, dy) in directions {
                    let nx = x as i64 + dx * distance;
                    let ny = y as i64 + dy * distance;
                    let sx = x0 as i64 + nx;
                    let sy = y0 as i64 + ny;
                    if !inside(nx, ny)
                        && sx >= 0
                        && sy >= 0
                        && sx < frame.width as i64
                        && sy < frame.height as i64
                    {
                        contrast |=
                            value.saturating_sub(brightness(sx as usize, sy as usize)) >= 60;
                    }
                }
                boundary += 1;
                contrasted += usize::from(contrast);
            }
        }
        boundary >= 10 && contrasted as f32 / boundary as f32 >= 0.65
    }

    /// Split a touching multi-digit component into individual glyph components via column projection valleys
    pub fn split_component(comp: DetectedComponent, scale: f32) -> Vec<DetectedComponent> {
        let w = comp.bbox.width as usize;
        let h = comp.bbox.height as usize;
        let aspect = w as f32 / h as f32;

        // If aspect is typical for a single digit, return as is
        // Single digits (even wide 0 or 8 or 3) have aspect <= 0.95.
        // Two touching digits have aspect >= 1.15.
        if aspect < 1.15 || w < (24.0 * scale) as usize {
            return vec![comp];
        }

        if let Some(glyphs) = crate::vision::matcher::DigitMatcher::split_number_component(&comp) {
            return glyphs;
        }

        // Compute column projection
        let mut col_proj = vec![0usize; w];
        for y in 0..h {
            for x in 0..w {
                if comp.mask[y * w + x] > 0 {
                    col_proj[x] += 1;
                }
            }
        }

        let min_digit_w = (h as f32 * 0.28).max(5.0) as usize;
        let mut splits = Vec::new();

        let mut in_valley = false;
        let mut best_valley_x = 0;
        let mut min_val = usize::MAX;

        for x in min_digit_w..(w.saturating_sub(min_digit_w)) {
            let val = col_proj[x];
            if val <= (h as f32 * 0.40) as usize {
                if val < min_val {
                    min_val = val;
                    best_valley_x = x;
                }
                in_valley = true;
            } else if in_valley {
                if best_valley_x > 0 {
                    let last_split = splits.last().copied().unwrap_or(0);
                    if best_valley_x - last_split >= min_digit_w && w - best_valley_x >= min_digit_w
                    {
                        splits.push(best_valley_x);
                    }
                }
                min_val = usize::MAX;
                best_valley_x = 0;
                in_valley = false;
            }
        }
        if in_valley && best_valley_x > 0 {
            let last_split = splits.last().copied().unwrap_or(0);
            if best_valley_x - last_split >= min_digit_w && w - best_valley_x >= min_digit_w {
                splits.push(best_valley_x);
            }
        }

        if splits.is_empty() {
            return vec![comp];
        }

        let mut result = Vec::new();
        let mut prev_x = 0;
        let mut boundaries = splits;
        boundaries.push(w);

        for bx in boundaries {
            let sub_w = bx - prev_x;
            if sub_w < 4 {
                prev_x = bx;
                continue;
            }

            let mut min_gx = usize::MAX;
            let mut max_gx = 0;
            let mut min_gy = usize::MAX;
            let mut max_gy = 0;
            let mut sub_pixels = 0;

            for y in 0..h {
                for x in 0..sub_w {
                    if comp.mask[y * w + (prev_x + x)] > 0 {
                        min_gx = min_gx.min(x);
                        max_gx = max_gx.max(x);
                        min_gy = min_gy.min(y);
                        max_gy = max_gy.max(y);
                        sub_pixels += 1;
                    }
                }
            }

            if sub_pixels >= (8.0 * scale) as usize && min_gx <= max_gx && min_gy <= max_gy {
                let tight_w = (max_gx - min_gx + 1) as u32;
                let tight_h = (max_gy - min_gy + 1) as u32;
                let mut sub_mask = vec![0u8; (tight_w * tight_h) as usize];
                for y in min_gy..=max_gy {
                    for x in min_gx..=max_gx {
                        if comp.mask[y * w + (prev_x + x)] > 0 {
                            sub_mask[(y - min_gy) * tight_w as usize + (x - min_gx)] = 255;
                        }
                    }
                }

                result.push(DetectedComponent {
                    bbox: BoundingBox {
                        x: comp.bbox.x + (prev_x + min_gx) as u32,
                        y: comp.bbox.y + min_gy as u32,
                        width: tight_w,
                        height: tight_h,
                    },
                    element: comp.element,
                    label: comp.label,
                    pixel_count: sub_pixels,
                    mask: sub_mask,
                });
            }

            prev_x = bx;
        }

        if result.is_empty() {
            vec![comp]
        } else {
            result
        }
    }

    /// Two numbers that end up side by side form one row of glyphs: a 25286
    /// next to a 95922 was one ten-digit cluster, a 9942 whose neighbor
    /// covered the gap a seven-digit one. The digits of one number share a
    /// top line and a baseline, and a second number rarely sits at the same
    /// height and size. Cut the row where the glyphs on both sides agree
    /// among themselves and the two sides differ by a clear step. Each side
    /// keeps at least two glyphs; the caller accepts the cut only if both
    /// sides read as numbers, so a damaged glyph never shortens a number.
    pub fn split_row_at_step(
        cluster: &DamageCluster,
        scale: f32,
    ) -> Option<(DamageCluster, DamageCluster)> {
        let glyphs = &cluster.glyphs;
        if glyphs.len() < 4 {
            return None;
        }
        let top = |g: &DetectedComponent| g.bbox.y as i32;
        let bottom = |g: &DetectedComponent| (g.bbox.y + g.bbox.height) as i32;
        // (smallest, largest, median) of one edge over a run of glyphs.
        let spread = |part: &[DetectedComponent], edge: &dyn Fn(&DetectedComponent) -> i32| {
            let mut values: Vec<i32> = part.iter().map(edge).collect();
            values.sort_unstable();
            (
                values[0],
                values[values.len() - 1],
                values[values.len() / 2],
            )
        };
        let mut best: Option<(i32, usize)> = None;
        for cut in 2..=glyphs.len() - 2 {
            let (left, right) = glyphs.split_at(cut);
            let height = left
                .iter()
                .chain(right)
                .map(|g| g.bbox.height)
                .max()
                .unwrap_or(0) as f32;
            let needed = (height * 0.12).max(3.0 * scale);
            for edge in [&top as &dyn Fn(&DetectedComponent) -> i32, &bottom] {
                let (l_min, l_max, l_mid) = spread(left, edge);
                let (r_min, r_max, r_mid) = spread(right, edge);
                let step = (l_mid - r_mid).abs();
                // The sides must not reach into each other's range either.
                let apart = (r_min - l_max).max(l_min - r_max);
                if step as f32 >= needed
                    && ((l_max - l_min).max(r_max - r_min) as f32) <= step as f32 * 0.4
                    && apart as f32 >= step as f32 * 0.5
                    && best.is_none_or(|(s, _)| step > s)
                {
                    best = Some((step, cut));
                }
            }
        }
        let (_, cut) = best?;
        let part = |glyphs: &[DetectedComponent]| {
            let bbox = glyphs
                .iter()
                .skip(1)
                .fold(glyphs[0].bbox, |b, g| b.merge(&g.bbox));
            let crit_height = (27.0 * scale) as u32;
            DamageCluster {
                element: cluster.element,
                bbox,
                is_crit: glyphs.iter().any(|g| g.bbox.height >= crit_height),
                glyphs: glyphs.to_vec(),
            }
        };
        Some((part(&glyphs[..cut]), part(&glyphs[cut..])))
    }

    /// Check if a component is an exclamation mark '!' (narrow and has vertical gap before dot)
    pub fn is_exclamation_glyph(c: &DetectedComponent) -> bool {
        let gw = c.bbox.width as usize;
        let gh = c.bbox.height as usize;
        if gw == 0 || gh < 18 {
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
        Self::cluster_components_scaled(comps, 1.0)
    }

    /// Group components using explicit resolution scale factor
    pub fn cluster_components_scaled(
        comps: Vec<DetectedComponent>,
        scale: f32,
    ) -> Vec<DamageCluster> {
        let comps = crate::vision::matcher::DigitMatcher::merge_vertical_fragments(comps, scale);
        // Expand any multi-digit components into separated glyphs
        let mut individual_glyphs = Vec::with_capacity(comps.len() * 2);
        for c in comps {
            let split = Self::split_component(c, scale);
            for s in split {
                let aspect = s.bbox.width as f32 / s.bbox.height as f32;
                let min_h = (12.0 * scale) as u32;
                let max_h = (max_glyph_height(s.label) * scale) as u32;
                let min_w = (4.0 * scale) as u32;
                let max_w = (max_glyph_height(s.label) * 0.91 * scale) as u32;
                let min_px = (8.0 * scale) as usize;

                if s.bbox.height >= min_h
                    && s.bbox.height <= max_h
                    && s.bbox.width >= min_w
                    && s.bbox.width <= max_w
                    && s.pixel_count >= min_px
                    && aspect >= 0.16
                    && aspect <= 1.10
                {
                    individual_glyphs.push(s);
                }
            }
        }

        let mut sorted = individual_glyphs;
        sorted.sort_by_key(|c| c.bbox.x);

        let mut raw_clusters: Vec<Vec<DetectedComponent>> = Vec::new();

        for c in sorted {
            let mut matched_cluster = None;
            for cluster in raw_clusters.iter_mut() {
                let last = cluster.last().unwrap();
                if last.label != c.label {
                    continue;
                }

                let is_last_exclamation = Self::is_exclamation_glyph(last);
                if is_last_exclamation {
                    continue;
                }

                let dy = (last.bbox.y as i32 - c.bbox.y as i32).abs();
                let dh = (last.bbox.height as i32 - c.bbox.height as i32).abs();
                let max_h = last.bbox.height.max(c.bbox.height) as i32;

                let is_curr_exclamation = Self::is_exclamation_glyph(&c);
                let max_allowed_dy = if is_curr_exclamation {
                    (max_h as f32 * 0.50) as i32
                } else {
                    (max_h as f32 * 0.40) as i32
                };
                let max_allowed_dh = if is_curr_exclamation {
                    (max_h as f32 * 0.55) as i32
                } else {
                    (max_h as f32 * 0.40) as i32
                };

                if dy <= max_allowed_dy && dh <= max_allowed_dh {
                    let gap = c.bbox.x as i32 - (last.bbox.x + last.bbox.width) as i32;
                    let min_gap = (-8.0 * scale) as i32;
                    let max_gap = (max_h as f32 * 0.70) as i32;
                    if gap >= min_gap && gap <= max_gap {
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

            // Health bar rejection: a thin horizontal segmented bar where all slices are narrow sticks
            let total_w: u32 = glyphs.iter().map(|g| g.bbox.width).sum();
            let avg_asp = total_w as f32 / (glyphs.len() as f32 * glyphs[0].bbox.height as f32);
            if glyphs.len() >= 3 && avg_asp < 0.35 && glyphs[0].bbox.height <= (18.0 * scale) as u32
            {
                continue;
            }

            let element = glyphs[0].element;
            let mut bbox = glyphs[0].bbox;
            let mut is_crit = false;

            let crit_height_threshold = (27.0 * scale) as u32;
            for g in &glyphs {
                bbox = bbox.merge(&g.bbox);
                if g.bbox.height >= crit_height_threshold {
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
    fn relative_outline_accepts_gray_shadow_but_rejects_flat_white() {
        let mut frame = RawFrame {
            width: 32,
            height: 32,
            stride: 128,
            data: vec![170; 32 * 32 * 4],
            screen_x: 0,
            screen_y: 0,
        };
        let mask = vec![255; 8 * 16];
        for y in 8..24 {
            for x in 8..16 {
                frame.data[y * frame.stride + x * 4..y * frame.stride + x * 4 + 3].fill(250);
            }
        }
        assert!(!ColorFilter::has_dark_outline(&frame, 8, 15, 8, 23));
        assert!(ColorFilter::has_contrasting_stroke(
            &frame, 8, 8, 8, 16, &mask
        ));
        for pixel in frame.data.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[255, 80, 20, 255]);
        }
        for y in 8..24 {
            for x in 8..16 {
                frame.data[y * frame.stride + x * 4..y * frame.stride + x * 4 + 3].fill(250);
            }
        }
        assert!(
            ColorFilter::has_contrasting_stroke(&frame, 8, 8, 8, 16, &mask),
            "White text contrasts with saturated blue even when its blue channel is 255"
        );
        frame.data.fill(250);
        assert!(!ColorFilter::has_contrasting_stroke(
            &frame, 8, 8, 8, 16, &mask
        ));
    }

    #[test]
    fn dendro_text_is_cut_out_of_pale_green_glow() {
        // Lunar-Bloom covers the fight in a pale green glow that is Dendro by
        // hue. Saturated text inside it must stay a component of its own.
        let (w, h) = (640usize, 360usize);
        let mut frame = RawFrame {
            width: w as u32,
            height: h as u32,
            stride: w * 4,
            data: vec![0; w * h * 4],
            screen_x: 0,
            screen_y: 0,
        };
        let mut fill = |x0: usize, y0: usize, bw: usize, bh: usize, rgb: (u8, u8, u8)| {
            for y in y0..y0 + bh {
                for x in x0..x0 + bw {
                    let p = y * w * 4 + x * 4;
                    frame.data[p..p + 4].copy_from_slice(&[rgb.2, rgb.1, rgb.0, 255]);
                }
            }
        };
        // Colors measured in Lunar-Bloom footage: glow, then text.
        assert_eq!(classify_element(150, 214, 184), Some(ElementType::Dendro));
        assert_eq!(classify_element(0, 202, 80), Some(ElementType::Dendro));
        fill(200, 120, 240, 120, (150, 214, 184));
        fill(300, 160, 14, 30, (0, 202, 80));
        fill(322, 160, 14, 30, (0, 202, 80));

        let text: Vec<_> = ColorFilter::segment_frame(&frame)
            .into_iter()
            .filter(|c| c.element == ElementType::Dendro && c.label == label_pixel(0, 202, 80))
            .map(|c| (c.bbox.x, c.bbox.y, c.bbox.width, c.bbox.height))
            .collect();
        assert_eq!(text, [(300, 160, 14, 30), (322, 160, 14, 30)]);
    }

    #[test]
    fn lunar_bloom_gradient_gets_its_own_label() {
        // Top, middle and lower part of a Lunar-Bloom digit as recorded.
        for (r, g, b) in [(185, 253, 180), (203, 253, 190), (221, 251, 217)] {
            assert_eq!(label_pixel(r, g, b), LUNAR_BLOOM_LABEL, "{r},{g},{b}");
        }
        assert_eq!(
            element_of_label(LUNAR_BLOOM_LABEL),
            Some(ElementType::Dendro)
        );
        // The white end of the gradient and ordinary text keep their labels.
        let physical = label_pixel(245, 245, 250);
        assert_eq!(element_of_label(physical), Some(ElementType::Physical));
        assert_eq!(label_pixel(250, 253, 239), physical);
        assert_eq!(label_pixel(249, 255, 240), physical);
        assert_ne!(label_pixel(0, 202, 80), LUNAR_BLOOM_LABEL);
        // Glow is too dim to be mistaken for the text.
        assert_ne!(label_pixel(150, 214, 184), LUNAR_BLOOM_LABEL);
    }

    #[test]
    fn lunar_bloom_digit_is_completed_and_straightened() {
        // A leaning bar, 8 px wide and 40 px tall: green for the upper 28
        // rows, white below, on a dark background.
        let (w, h) = (640usize, 360usize);
        let mut frame = RawFrame {
            width: w as u32,
            height: h as u32,
            stride: w * 4,
            data: vec![0; w * h * 4],
            screen_x: 0,
            screen_y: 0,
        };
        let draw = |frame: &mut RawFrame, left: usize, top: usize| {
            for y in 0..40usize {
                let x0 = left + (LUNAR_SLANT * (39 - y) as f32).round() as usize;
                let rgb: [u8; 3] = if y < 28 {
                    [185, 253, 180]
                } else {
                    [255, 255, 255]
                };
                for x in x0..x0 + 8 {
                    let p = (top + y) * w * 4 + x * 4;
                    frame.data[p..p + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
                }
            }
        };
        // Close enough that the leaning boxes overlap.
        draw(&mut frame, 300, 160);
        draw(&mut frame, 312, 160);

        let glyphs: Vec<_> = ColorFilter::segment_frame(&frame)
            .into_iter()
            .filter(|c| c.label == LUNAR_BLOOM_LABEL)
            .collect();
        assert_eq!(glyphs.len(), 2, "the bars stay separate");
        for (glyph, left) in glyphs.iter().zip([300u32, 312]) {
            // Whole height including the white end, and upright: about as
            // wide as the bar itself instead of bar plus lean (18 px). Row
            // shifts are rounded, which may cost one pixel.
            assert_eq!((glyph.bbox.y, glyph.bbox.height), (160, 40));
            assert!((8..=9).contains(&glyph.bbox.width), "{:?}", glyph.bbox);
            assert!(glyph.bbox.x.abs_diff(left) <= 1, "{:?}", glyph.bbox);
            assert_eq!(glyph.pixel_count, 8 * 40);
        }
    }

    #[test]
    fn label_table_matches_direct_classification() {
        let table = label_table();
        // A coarse sweep plus every value near the brightness cut.
        for r in (0..=255u8).step_by(3) {
            for g in (0..=255u8).step_by(5) {
                for b in (0..=255u8).step_by(7).chain(145..=150) {
                    assert_eq!(table.label(r, g, b), label_pixel(r, g, b), "{r},{g},{b}");
                }
            }
        }
        assert!(table.mixed.len() < 8 << 20, "most blocks should be uniform");
    }

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
        assert_eq!(classify_element(255, 153, 4), Some(ElementType::Pyro));
        // Anemo (Mint/Teal)
        assert_eq!(classify_element(40, 240, 190), Some(ElementType::Anemo));
        // Physical (Bright White)
        assert_eq!(classify_element(245, 245, 250), Some(ElementType::Physical));
    }
}
