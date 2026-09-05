use crate::vision::filter::DamageCluster;
use crate::vision::matcher::RecognizedHit;
use image::RgbImage;
use ocrs::{ImageSource, OcrEngine, OcrEngineParams};
use rten::Model;
use std::sync::Mutex;

pub struct MlMatcher {
    engine: Mutex<OcrEngine>,
}

fn load_model_file(filename: &str) -> Result<Vec<u8>, String> {
    let candidates = [
        format!("src-tauri/assets/{}", filename),
        format!("assets/{}", filename),
        format!("../assets/{}", filename),
        format!("../../assets/{}", filename),
    ];
    for p in &candidates {
        if let Ok(bytes) = std::fs::read(p) {
            return Ok(bytes);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if let Ok(bytes) = std::fs::read(dir.join("assets").join(filename)) {
                return Ok(bytes);
            }
            if let Ok(bytes) = std::fs::read(dir.join(filename)) {
                return Ok(bytes);
            }
        }
    }
    Err(format!("Failed to locate model asset '{}' in any candidate path", filename))
}

impl MlMatcher {
    pub fn new() -> Result<Self, String> {
        let rec_model = load_model_file("text-recognition.rten")?;
        let det_model = load_model_file("text-detection.rten")?;

        let engine = OcrEngine::new(OcrEngineParams {
            detection_model: Some(Model::load(det_model).map_err(|e| format!("Failed to parse det model: {}", e))?),
            recognition_model: Some(Model::load(rec_model).map_err(|e| format!("Failed to parse rec model: {}", e))?),
            ..Default::default()
        }).map_err(|e| format!("Failed to create OcrEngine: {}", e))?;
        Ok(Self {
            engine: Mutex::new(engine),
        })
    }

    pub fn recognize_cluster(&self, cluster: &DamageCluster) -> Option<RecognizedHit> {
        if cluster.glyphs.is_empty() || cluster.bbox.width == 0 || cluster.bbox.height == 0 {
            return None;
        }

        // Add padding around the text
        let padding: u32 = 10;
        let mut img = RgbImage::from_pixel(
            cluster.bbox.width + padding * 2,
            cluster.bbox.height + padding * 2,
            image::Rgb([0, 0, 0]),
        );

        for glyph in &cluster.glyphs {
            let dx = glyph.bbox.x - cluster.bbox.x + padding;
            let dy = glyph.bbox.y - cluster.bbox.y + padding;
            for y in 0..glyph.bbox.height {
                for x in 0..glyph.bbox.width {
                    let px = glyph.mask[(y * glyph.bbox.width + x) as usize];
                    if px > 0 {
                        // ocrs works best with black text on white background
                        img.put_pixel(dx + x, dy + y, image::Rgb([255, 255, 255]));
                    }
                }
            }
        }

        // Convert the background to white, and text to black
        for pixel in img.pixels_mut() {
            if pixel[0] == 0 {
                *pixel = image::Rgb([255, 255, 255]);
            } else {
                *pixel = image::Rgb([0, 0, 0]);
            }
        }

        let dyn_img = image::DynamicImage::ImageRgb8(img);
        let img_source = match ImageSource::from_bytes(dyn_img.as_bytes(), (dyn_img.width(), dyn_img.height())) {
            Ok(src) => src,
            Err(e) => {
                println!("ImageSource error: {:?}", e);
                return None;
            }
        };

        let engine = self.engine.lock().unwrap();
        let ocr_input = match engine.prepare_input(img_source) {
            Ok(input) => input,
            Err(_) => return None,
        };
        let text = match engine.get_text(&ocr_input) {
            Ok(text) => text,
            Err(_) => return None,
        };
        println!("OCR Raw Text: {:?} for cluster at ({},{})", text, cluster.bbox.x, cluster.bbox.y);

        // Rejection rule 1: Text must not contain alphabetic characters (no words, enemy names, reactions)
        if text.chars().any(|c| c.is_alphabetic()) {
            return None;
        }

        let digits: String = text.chars().filter(|c| c.is_digit(10)).collect();
        // Rejection rule 2: Genshin damage numbers are at least 2 digits (e.g. 10 to 999,999)
        if digits.len() < 2 {
            return None;
        }

        // Rejection rule 3: Expected digit count must match the number of clustered glyphs
        let min_expected_digits = if cluster.is_crit {
            cluster.glyphs.len().saturating_sub(1)
        } else {
            cluster.glyphs.len()
        };

        if digits.len() < min_expected_digits {
            return None;
        }

        let parsed_value = match digits.parse::<u32>() {
            Ok(v) if v >= 10 => v,
            _ => return None,
        };

        Some(RecognizedHit {
            value: parsed_value,
            is_crit: cluster.is_crit,
            confidence: 1.0,
            x: cluster.bbox.x as i32,
            y: cluster.bbox.y as i32,
        })
    }
}
