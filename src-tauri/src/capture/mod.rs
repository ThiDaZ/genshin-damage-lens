#[derive(Clone)]
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    /// BGRA8 or RGBA8 pixel buffer
    pub data: Vec<u8>,
    pub screen_x: i32,
    pub screen_y: i32,
}

#[cfg(windows)]
pub mod dxgi;
pub mod window_finder;

pub trait CaptureBackend: Send + Sync {
    fn capture_frame(&mut self) -> Result<Option<RawFrame>, String>;
}

pub struct ScreenCapture {
    #[cfg(windows)]
    backend: Option<dxgi::DxgiCapture>,
    synthetic_frame: Option<RawFrame>,
}

impl ScreenCapture {
    pub fn new() -> Self {
        #[cfg(windows)]
        {
            let backend = dxgi::DxgiCapture::new().ok();
            Self {
                backend,
                synthetic_frame: None,
            }
        }
        #[cfg(not(windows))]
        {
            Self {
                synthetic_frame: None,
            }
        }
    }

    pub fn capture(&mut self) -> Option<RawFrame> {
        #[cfg(windows)]
        {
            if let Some(backend) = &mut self.backend {
                match backend.capture_frame() {
                    Ok(Some(frame)) => return Some(frame),
                    Ok(None) => return None,
                    Err(_) => {
                        // Attempt re-init on access lost
                        let _ = backend.reinitialize();
                        return None;
                    }
                }
            }
        }

        self.synthetic_frame.clone()
    }
}
