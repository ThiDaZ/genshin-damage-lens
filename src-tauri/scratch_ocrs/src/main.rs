use ocrs::{OcrEngine, OcrEngineParams, OcrInput};
use rten::Model;
use image::{GrayImage, Luma};

fn main() {
    let rec_model = std::fs::read("../assets/text-recognition.rten").unwrap();
    let engine = OcrEngine::new(OcrEngineParams {
        detection_model: None,
        recognition_model: Some(Model::load(&rec_model).unwrap()),
        ..Default::default()
    }).unwrap();
    println!("Engine initialized!");

    // Create a 64x64 dummy image
    let img = GrayImage::from_pixel(64, 64, Luma([0]));
    let dynamic_img = image::DynamicImage::ImageLuma8(img);
    let ocr_input = engine.prepare_input(dynamic_img.into_rgb8()).unwrap();
    let text = engine.get_text(&ocr_input).unwrap();
    for line in text {
        println!("Recognized: {}", line.to_string());
    }
}
