#[derive(Clone)]
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    /// BGRA8 pixel buffer; PNG/RGBA inputs must swap red and blue.
    pub data: Vec<u8>,
    pub screen_x: i32,
    pub screen_y: i32,
}

#[cfg(windows)]
pub mod dxgi;
pub mod window_finder;

pub trait CaptureBackend: Send + Sync {
    fn capture_frame(&mut self) -> Result<CaptureOutcome, String>;
}

pub enum CaptureOutcome {
    Frame(RawFrame),
    /// Desktop duplication timed out. This does not mean the game disappeared.
    NoNewFrame,
    /// No visible game, invalid crop, or a capture backend that needs recovery.
    Unavailable,
}

#[derive(Debug, PartialEq)]
pub(crate) struct FrameCrop {
    pub local_x: u32,
    pub local_y: u32,
    pub width: u32,
    pub height: u32,
    pub screen_x: i32,
    pub screen_y: i32,
}

/// Convert virtual-desktop coordinates to texture coordinates, including negative monitors.
pub(crate) fn clip_to_output(
    crop: window_finder::WindowRect,
    output: window_finder::WindowRect,
) -> Option<FrameCrop> {
    let left = i64::from(crop.x).max(i64::from(output.x));
    let top = i64::from(crop.y).max(i64::from(output.y));
    let right = (i64::from(crop.x) + i64::from(crop.width))
        .min(i64::from(output.x) + i64::from(output.width));
    let bottom = (i64::from(crop.y) + i64::from(crop.height))
        .min(i64::from(output.y) + i64::from(output.height));
    if right - left < 640 || bottom - top < 480 {
        return None;
    }
    Some(FrameCrop {
        local_x: (left - i64::from(output.x)) as u32,
        local_y: (top - i64::from(output.y)) as u32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
        screen_x: left as i32,
        screen_y: top as i32,
    })
}

pub struct ScreenCapture {
    #[cfg(windows)]
    backend: Option<dxgi::DxgiCapture>,
    #[cfg(windows)]
    retry_after: std::time::Instant,
}

impl ScreenCapture {
    pub fn new() -> Self {
        #[cfg(windows)]
        {
            let backend = dxgi::DxgiCapture::new().ok();
            Self {
                backend,
                retry_after: std::time::Instant::now(),
            }
        }
        #[cfg(not(windows))]
        {
            Self {}
        }
    }

    pub fn capture(&mut self) -> CaptureOutcome {
        #[cfg(windows)]
        {
            let now = std::time::Instant::now();
            if now < self.retry_after {
                return CaptureOutcome::Unavailable;
            }
            if self.backend.is_none() {
                self.backend = dxgi::DxgiCapture::new().ok();
                if self.backend.is_none() {
                    self.retry_after = now + std::time::Duration::from_secs(1);
                    return CaptureOutcome::Unavailable;
                }
            }
            if let Some(backend) = &mut self.backend {
                match backend.capture_frame() {
                    Ok(outcome) => return outcome,
                    Err(_) => {
                        // Recreate the device as well after access loss or monitor changes.
                        self.backend = None;
                        self.retry_after = now + std::time::Duration::from_secs(1);
                        return CaptureOutcome::Unavailable;
                    }
                }
            }
        }

        CaptureOutcome::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use window_finder::WindowRect;

    #[test]
    fn crops_on_negative_and_offset_monitors() {
        for (ox, oy) in [(-1920, -1080), (1920, 200)] {
            let output = WindowRect {
                x: ox,
                y: oy,
                width: 1920,
                height: 1080,
            };
            let game = WindowRect {
                x: ox + 100,
                y: oy + 50,
                width: 1280,
                height: 720,
            };
            assert_eq!(
                clip_to_output(game, output),
                Some(FrameCrop {
                    local_x: 100,
                    local_y: 50,
                    width: 1280,
                    height: 720,
                    screen_x: ox + 100,
                    screen_y: oy + 50
                })
            );
        }
    }

    #[test]
    fn clips_edges_and_rejects_nonoverlap() {
        let output = WindowRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let mut game = WindowRect {
            x: -100,
            y: -50,
            width: 1280,
            height: 720,
        };
        let crop = clip_to_output(game, output).unwrap();
        assert_eq!(
            (crop.width, crop.height, crop.local_x, crop.local_y),
            (1180, 670, 0, 0)
        );
        game.x = -5000;
        assert_eq!(clip_to_output(game, output), None);
    }
}
