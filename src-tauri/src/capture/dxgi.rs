use super::gpu_labels::{read_staging, staging_texture, GpuLabeler, LABEL_FORMAT};
use super::window_finder::{WindowFinder, WindowRect};
use super::{clip_to_output, CaptureOutcome, FrameCrop, RawFrame};
use windows::core::{Interface, Result};
use windows::Win32::Foundation::{E_FAIL, HMODULE};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1, IDXGIOutput, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};

/// Longest wait for the desktop to change before a call reports no new frame.
const FRAME_WAIT: std::time::Duration = std::time::Duration::from_millis(16);

pub struct DxgiCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    /// Two CPU-readable copies, used in turn. See `capture_region`.
    staging: [Option<ID3D11Texture2D>; 2],
    /// Labels pixels on the graphics card, with a CPU-readable copy of the
    /// labels beside each of the two frame copies. `Err` holds the reason
    /// when the shader path could not be set up on this device; frames are
    /// then labeled on the processor.
    labeling: std::result::Result<(GpuLabeler, [ID3D11Texture2D; 2]), String>,
    /// The user's choice: label on the graphics card when that is possible.
    use_gpu: bool,
    /// Name of the graphics adapter the display is attached to. Capture and
    /// labeling run there, which on a laptop with two adapters is usually
    /// not the one the game renders on.
    adapter_name: String,
    /// The staging copy written by the previous call, not yet read back.
    pending: Option<Pending>,
    next: usize,
    /// Buffers handed back by the caller, reused for the next frame.
    spare: Option<Vec<u8>>,
    spare_labels: Option<Vec<u8>>,
    /// Time spent copying and reading back the last delivered frame.
    last_copy: std::time::Duration,
    width: u32,
    height: u32,
    output_rect: WindowRect,
}

/// A frame that has been copied for reading but not read back yet.
struct Pending {
    index: usize,
    crop: FrameCrop,
    /// Its labels were computed too and wait in the label copy `index`.
    labeled: bool,
}

impl DxgiCapture {
    pub fn new() -> Result<Self> {
        let target = WindowFinder::find_genshin().and_then(WindowFinder::get_client_rect);
        let (adapter, output) = Self::select_output(target)?;
        let (device, context) = Self::create_d3d_device(&adapter)?;
        let adapter_name = unsafe { adapter.GetDesc() }
            .map(|desc| {
                let name = &desc.Description;
                let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
                String::from_utf16_lossy(&name[..len])
            })
            .unwrap_or_default();
        let output1: IDXGIOutput1 = output.cast()?;
        let duplication = unsafe { output1.DuplicateOutput(&device)? };
        let desc = unsafe { output.GetDesc()? };
        // DXGI rotated textures need a separate pixel transform. Reject them explicitly.
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED,
        };
        if desc.Rotation != DXGI_MODE_ROTATION_IDENTITY
            && desc.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED
        {
            return Err(windows::core::Error::new(
                E_FAIL,
                "Rotated display capture is not supported",
            ));
        }
        let rect = desc.DesktopCoordinates;
        let width = (rect.right - rect.left) as u32;
        let height = (rect.bottom - rect.top) as u32;
        // `GDL_NO_GPU_LABELS` makes the shader path count as unavailable, to
        // try the fallback on a machine where it works.
        let allowed = if std::env::var_os("GDL_NO_GPU_LABELS").is_some() {
            Err(windows::core::Error::new(
                E_FAIL,
                "switched off with GDL_NO_GPU_LABELS",
            ))
        } else {
            Ok(())
        };
        let labeling = allowed
            .and_then(|()| GpuLabeler::new(&device, &context, width, height))
            .and_then(|labeler| {
                let copies = [
                    staging_texture(&device, width, height, LABEL_FORMAT)?,
                    staging_texture(&device, width, height, LABEL_FORMAT)?,
                ];
                Ok((labeler, copies))
            })
            .map_err(|error| error.message());
        let mut instance = Self {
            device,
            context,
            duplication,
            staging: [None, None],
            labeling,
            use_gpu: true,
            adapter_name,
            pending: None,
            next: 0,
            spare: None,
            spare_labels: None,
            last_copy: std::time::Duration::ZERO,
            width,
            height,
            output_rect: WindowRect {
                x: rect.left,
                y: rect.top,
                width,
                height,
            },
        };
        instance.ensure_staging_texture(width, height)?;
        Ok(instance)
    }

    fn select_output(target: Option<WindowRect>) -> Result<(IDXGIAdapter, IDXGIOutput)> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut fallback = None;
            let mut ai = 0;
            while let Ok(adapter) = factory.EnumAdapters1(ai) {
                let mut oi = 0;
                while let Ok(output) = adapter.EnumOutputs(oi) {
                    let desc = output.GetDesc()?;
                    if desc.AttachedToDesktop.as_bool() {
                        let adapter: IDXGIAdapter = adapter.cast()?;
                        if fallback.is_none() {
                            fallback = Some((adapter.clone(), output.clone()));
                        }
                        if let Some(target) = target {
                            let cx = i64::from(target.x) + i64::from(target.width) / 2;
                            let cy = i64::from(target.y) + i64::from(target.height) / 2;
                            let r = desc.DesktopCoordinates;
                            if cx >= i64::from(r.left)
                                && cx < i64::from(r.right)
                                && cy >= i64::from(r.top)
                                && cy < i64::from(r.bottom)
                            {
                                return Ok((adapter, output));
                            }
                        }
                    }
                    oi += 1;
                }
                ai += 1;
            }
            fallback.ok_or_else(|| windows::core::Error::new(E_FAIL, "No attached capture output"))
        }
    }

    fn create_d3d_device(adapter: &IDXGIAdapter) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
        }
        match (device, context) {
            (Some(device), Some(context)) => Ok((device, context)),
            _ => Err(windows::core::Error::new(
                E_FAIL,
                "D3D device initialization returned no device",
            )),
        }
    }

    fn ensure_staging_texture(&mut self, width: u32, height: u32) -> Result<()> {
        if self.staging.iter().all(Option::is_some) && self.width == width && self.height == height
        {
            return Ok(());
        }

        self.pending = None;
        for slot in &mut self.staging {
            *slot = Some(staging_texture(
                &self.device,
                width,
                height,
                DXGI_FORMAT_B8G8R8A8_UNORM,
            )?);
        }
        self.width = width;
        self.height = height;
        Ok(())
    }

    /// The desktop rectangle of the display being captured.
    pub fn output_rect(&self) -> WindowRect {
        self.output_rect
    }

    /// Give a delivered frame's buffers back for reuse. Allocating eight
    /// megabytes per frame costs a page fault for every page, every frame.
    pub fn recycle(&mut self, frame: RawFrame) {
        self.spare = Some(frame.data);
        if frame.labels.is_some() {
            self.spare_labels = frame.labels;
        }
    }

    /// Choose where pixels are labeled: on the graphics card (the default)
    /// or on the processor. Takes effect with the next captured frame.
    pub fn set_gpu_labels(&mut self, enabled: bool) {
        self.use_gpu = enabled;
    }

    /// Whether new frames are labeled on the graphics card.
    pub fn gpu_labels(&self) -> bool {
        self.use_gpu && self.labeling.is_ok()
    }

    /// The graphics adapter that captures the display and labels pixels.
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// Why labeling on the graphics card is not possible, if it is not.
    pub fn gpu_labels_error(&self) -> Option<&str> {
        self.labeling.as_ref().err().map(String::as_str)
    }

    /// Copy and read-back time of the most recently delivered frame.
    pub fn last_copy(&self) -> std::time::Duration {
        self.last_copy
    }

    pub fn capture_frame(&mut self) -> Result<CaptureOutcome> {
        // Check game availability independently of whether the desktop changed.
        match WindowFinder::find_genshin().and_then(WindowFinder::get_client_rect) {
            Some(rect) => self.capture_region(rect),
            None => {
                self.pending = None;
                Ok(CaptureOutcome::Unavailable)
            }
        }
    }

    /// Capture the part of the display covered by `crop` (desktop coordinates).
    ///
    /// Reading a texture back right after copying it makes the CPU wait until
    /// the GPU has executed the copy. With a game keeping the GPU busy that
    /// wait is most of a frame. So each call copies the new desktop image into
    /// one staging texture and reads back the other, which was copied a call
    /// earlier and is finished by now. Frames arrive one call late and nothing
    /// waits. The first call after a start or a pause therefore yields no frame.
    pub fn capture_region(&mut self, crop: WindowRect) -> Result<CaptureOutcome> {
        let cx = i64::from(crop.x) + i64::from(crop.width) / 2;
        let cy = i64::from(crop.y) + i64::from(crop.height) / 2;
        if cx < i64::from(self.output_rect.x)
            || cy < i64::from(self.output_rect.y)
            || cx >= i64::from(self.output_rect.x) + i64::from(self.output_rect.width)
            || cy >= i64::from(self.output_rect.y) + i64::from(self.output_rect.height)
        {
            return Err(windows::core::Error::new(
                E_FAIL,
                "Game moved to another capture output",
            ));
        }
        let crop = match clip_to_output(crop, self.output_rect) {
            Some(crop) => crop,
            None => {
                self.pending = None;
                return Ok(CaptureOutcome::Unavailable);
            }
        };
        // Desktop duplication also reports every movement of the mouse
        // pointer, as a frame whose image has not changed. Copying and reading
        // the whole desktop again for each of those doubled the work whenever
        // the pointer moved, on the processor and on the graphics card the
        // game and a recorder are already competing for. Such frames are
        // released at once, and the wait goes on for a real one.
        let deadline = std::time::Instant::now() + FRAME_WAIT;
        let resource = loop {
            let wait = deadline.saturating_duration_since(std::time::Instant::now());
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            let hr = unsafe {
                self.duplication.AcquireNextFrame(
                    wait.as_millis() as u32,
                    &mut frame_info,
                    &mut resource,
                )
            };
            if let Err(e) = hr {
                if e.code() == DXGI_ERROR_WAIT_TIMEOUT {
                    // The desktop is still; hand out the copy that is waiting.
                    return self.read_pending();
                }
                return Err(e);
            }
            if frame_info.LastPresentTime != 0 {
                break resource;
            }
            drop(resource);
            let _ = unsafe { self.duplication.ReleaseFrame() };
            if std::time::Instant::now() >= deadline {
                return self.read_pending();
            }
        };
        let started = std::time::Instant::now();

        let resource = match resource {
            Some(r) => r,
            None => {
                let _ = unsafe { self.duplication.ReleaseFrame() };
                return self.read_pending();
            }
        };

        let desktop_texture: ID3D11Texture2D = match resource.cast() {
            Ok(t) => t,
            Err(e) => {
                let _ = unsafe { self.duplication.ReleaseFrame() };
                return Err(e);
            }
        };

        let mut texture_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            desktop_texture.GetDesc(&mut texture_desc);
        }
        if texture_desc.Width != self.width || texture_desc.Height != self.height {
            let _ = unsafe { self.duplication.ReleaseFrame() };
            return Err(windows::core::Error::new(E_FAIL, "Capture output resized"));
        }
        let Some(staging) = self.staging[self.next].clone() else {
            let _ = unsafe { self.duplication.ReleaseFrame() };
            return Ok(CaptureOutcome::Unavailable);
        };

        // Queue the work and submit it; do not wait for it.
        let labeling = self
            .labeling
            .as_ref()
            .ok()
            .filter(|_| self.use_gpu && texture_desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM);
        let labeled = labeling.is_some();
        unsafe {
            match labeling {
                Some((labeler, copies)) => {
                    // The labels and the pixels both come from the labeler's
                    // private copy of the desktop image, so they describe
                    // the same picture whatever the desktop does next.
                    labeler.label(&desktop_texture);
                    self.context.CopyResource(&staging, labeler.image());
                    self.context
                        .CopyResource(&copies[self.next], labeler.labels());
                }
                None => self.context.CopyResource(&staging, &desktop_texture),
            }
            self.context.Flush();
        }
        let _ = unsafe { self.duplication.ReleaseFrame() };

        let ready = self.pending.replace(Pending {
            index: self.next,
            crop,
            labeled,
        });
        self.next ^= 1;
        let outcome = match ready {
            Some(frame) => self.read_back(&frame)?,
            None => CaptureOutcome::NoNewFrame,
        };
        self.last_copy = started.elapsed();
        Ok(outcome)
    }

    fn read_pending(&mut self) -> Result<CaptureOutcome> {
        match self.pending.take() {
            Some(frame) => {
                let started = std::time::Instant::now();
                let outcome = self.read_back(&frame)?;
                self.last_copy = started.elapsed();
                Ok(outcome)
            }
            None => Ok(CaptureOutcome::NoNewFrame),
        }
    }

    /// Map a finished staging copy and take the crop out of it, with the
    /// crop's labels if they were computed for this frame.
    fn read_back(&mut self, frame: &Pending) -> Result<CaptureOutcome> {
        let crop = &frame.crop;
        let Some(staging) = self.staging[frame.index].clone() else {
            return Ok(CaptureOutcome::Unavailable);
        };
        if crop.width < 640 || crop.height < 480 {
            return Ok(CaptureOutcome::Unavailable);
        }
        let origin = (crop.local_x as usize, crop.local_y as usize);
        let size = (crop.width as usize, crop.height as usize);

        let mut data = self.spare.take().unwrap_or_default();
        read_staging(&self.context, &staging, origin, size, 4, &mut data)?;
        let labels = match (&self.labeling, frame.labeled) {
            (Ok((_, copies)), true) => {
                let mut labels = self.spare_labels.take().unwrap_or_default();
                let copy = &copies[frame.index];
                read_staging(&self.context, copy, origin, size, 1, &mut labels)?;
                Some(labels)
            }
            _ => None,
        };

        Ok(CaptureOutcome::Frame(RawFrame {
            width: crop.width,
            height: crop.height,
            stride: size.0 * 4,
            data,
            screen_x: crop.screen_x,
            screen_y: crop.screen_y,
            labels,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vision::filter::label_bgra;
    use std::time::{Duration, Instant};

    /// Frames of the real desktop, labeled on the graphics card, against the
    /// processor's labels of the same pixels. Needs a desktop to duplicate
    /// and is skipped where there is none: a build server, a locked or
    /// sleeping screen.
    #[test]
    fn desktop_frames_come_with_the_processors_labels() {
        let mut capture = match DxgiCapture::new() {
            Ok(capture) => capture,
            Err(error) => {
                eprintln!("skipped: no desktop to capture ({error})");
                return;
            }
        };
        if let Some(error) = capture.gpu_labels_error() {
            eprintln!("skipped: no labeling on this graphics device ({error})");
            return;
        }
        // A crop at an odd offset, so that the label rows are taken from the
        // same place in their texture as the pixel rows are from theirs.
        let screen = capture.output_rect();
        let crop = WindowRect {
            x: screen.x + 37,
            y: screen.y + 21,
            width: screen.width.saturating_sub(101),
            height: screen.height.saturating_sub(53),
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        let (mut with_labels, mut without) = (0, 0);
        while Instant::now() < deadline && (with_labels < 6 || without < 2) {
            // After six labeled frames, switch to the processor: frames then
            // arrive without labels, and one that was already on its way
            // keeps the labels it was captured with.
            capture.set_gpu_labels(with_labels < 6);
            let frame = match capture.capture_region(crop) {
                Ok(CaptureOutcome::Frame(frame)) => frame,
                Ok(_) => continue,
                Err(error) => {
                    eprintln!("skipped: desktop capture stopped ({error})");
                    return;
                }
            };
            let (w, h) = (frame.width as usize, frame.height as usize);
            assert_eq!((w, h), (crop.width as usize, crop.height as usize));
            match &frame.labels {
                Some(labels) => {
                    assert!(
                        *labels == label_bgra(&frame.data, frame.stride, w, h),
                        "labels from the graphics card differ from the processor's"
                    );
                    with_labels += 1;
                }
                None => without += 1,
            }
            capture.recycle(frame);
        }
        // A still desktop produces a single frame; that one must have been
        // checked. No frame at all means there was nothing to look at.
        if with_labels == 0 {
            eprintln!("skipped: the desktop produced no frame");
        } else {
            eprintln!("{with_labels} desktop frames labeled identically, {without} without labels");
        }
    }
}
