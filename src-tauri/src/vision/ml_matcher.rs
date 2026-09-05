use crate::vision::filter::DamageCluster;
use crate::vision::matcher::RecognizedHit;
use image::RgbImage;
use ocrs::{ImageSource, OcrEngine, OcrEngineParams};
use rten::Model;
use std::sync::Mutex;

pub struct MlMatcher {
    engine: Mutex<OcrEngine>,
}

impl MlMatcher {
    pub fn new() -> Result<Self, String> {
        let rec_model = std::fs::read("assets/text-recognition.rten")
            .unwrap_or_else(|_| std::fs::read("../assets/text-recognition.rten").expect("Failed to load rec model"));
        let det_model = std::fs::read("assets/text-detection.rten")
            .unwrap_or_else(|_| std::fs::read("../assets/text-detection.rten").expect("Failed to load det model"));
            
        let engine = OcrEngine::new(OcrEngineParams {
            detection_model: Some(Model::load(det_model).unwrap()),
            recognition_model: Some(Model::load(rec_model).unwrap()),
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

        let mut parsed_value = 0;
        let mut confidence = 0.0;
        
        let digits: String = text.chars().filter(|c| c.is_digit(10)).collect();
        if let Ok(v) = digits.parse::<u32>() {
            parsed_value = v;
            confidence = 1.0; // Dummy confidence for now
        }

        if parsed_value > 0 {
            Some(RecognizedHit {
                value: parsed_value,
                is_crit: cluster.is_crit,
                confidence,
                x: cluster.bbox.x as i32,
                y: cluster.bbox.y as i32,
            })
        } else {
            None
        }
    }
}
