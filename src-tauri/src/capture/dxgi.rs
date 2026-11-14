use super::window_finder::{WindowFinder, WindowRect};
use super::{clip_to_output, CaptureOutcome, FrameCrop, RawFrame};
use windows::core::{Interface, Result};
use windows::Win32::Foundation::{E_FAIL, HMODULE};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1, IDXGIOutput, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};

pub struct DxgiCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    /// Two CPU-readable copies, used in turn. See `capture_region`.
    staging: [Option<ID3D11Texture2D>; 2],
    /// The staging texture written by the previous call, not yet read back.
    pending: Option<(usize, FrameCrop)>,
    next: usize,
    /// A pixel buffer handed back by the caller, reused for the next frame.
    spare: Option<Vec<u8>>,
    /// Time spent copying and reading back the last delivered frame.
    last_copy: std::time::Duration,
    width: u32,
    height: u32,
    output_rect: WindowRect,
}

impl DxgiCapture {
    pub fn new() -> Result<Self> {
        let target = WindowFinder::find_genshin().and_then(WindowFinder::get_client_rect);
        let (adapter, output) = Self::select_output(target)?;
        let (device, context) = Self::create_d3d_device(&adapter)?;
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
        let mut instance = Self {
            device,
            context,
            duplication,
            staging: [None, None],
            pending: None,
            next: 0,
            spare: None,
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

        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };

        self.pending = None;
        for slot in &mut self.staging {
            let mut tex = None;
            unsafe {
                self.device.CreateTexture2D(&desc, None, Some(&mut tex))?;
            }
            *slot = tex;
        }
        self.width = width;
        self.height = height;
        Ok(())
    }

    /// The desktop rectangle of the display being captured.
    pub fn output_rect(&self) -> WindowRect {
        self.output_rect
    }

    /// Give a delivered frame's pixel buffer back for reuse. Allocating eight
    /// megabytes per frame costs a page fault for every page, every frame.
    pub fn recycle(&mut self, data: Vec<u8>) {
        self.spare = Some(data);
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
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;

        let hr = unsafe {
            self.duplication
                .AcquireNextFrame(16, &mut frame_info, &mut resource)
        };

        if let Err(e) = hr {
            if e.code() == DXGI_ERROR_WAIT_TIMEOUT {
                // The desktop is still; hand out the copy that is waiting.
                return self.read_pending();
            }
            return Err(e);
        }
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

        // Queue the copy and submit it; do not wait for it.
        unsafe {
            self.context.CopyResource(&staging, &desktop_texture);
            self.context.Flush();
        }
        let _ = unsafe { self.duplication.ReleaseFrame() };

        let ready = self.pending.replace((self.next, crop));
        self.next ^= 1;
        let outcome = match ready {
            Some((index, crop)) => self.read_back(index, &crop)?,
            None => CaptureOutcome::NoNewFrame,
        };
        self.last_copy = started.elapsed();
        Ok(outcome)
    }

    fn read_pending(&mut self) -> Result<CaptureOutcome> {
        match self.pending.take() {
            Some((index, crop)) => {
                let started = std::time::Instant::now();
                let outcome = self.read_back(index, &crop)?;
                self.last_copy = started.elapsed();
                Ok(outcome)
            }
            None => Ok(CaptureOutcome::NoNewFrame),
        }
    }

    /// Map a finished staging copy and take the crop out of it.
    fn read_back(&mut self, index: usize, crop: &FrameCrop) -> Result<CaptureOutcome> {
        let Some(staging) = self.staging[index].clone() else {
            return Ok(CaptureOutcome::Unavailable);
        };
        if crop.width < 640 || crop.height < 480 {
            return Ok(CaptureOutcome::Unavailable);
        }
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        }

        let p_data = mapped.pData as *const u8;
        let row_pitch = mapped.RowPitch as usize;
        let row_len = crop.width as usize * 4;
        let len = row_len * crop.height as usize;

        // Every byte is overwritten below, so a reused buffer needs no clearing.
        let mut data = self.spare.take().unwrap_or_default();
        data.resize(len, 0);
        for (row, out) in data.chunks_exact_mut(row_len).enumerate() {
            let src_offset = (crop.local_y as usize + row) * row_pitch + crop.local_x as usize * 4;
            unsafe {
                std::ptr::copy_nonoverlapping(p_data.add(src_offset), out.as_mut_ptr(), row_len);
            }
        }

        unsafe {
            self.context.Unmap(&staging, 0);
        }

        Ok(CaptureOutcome::Frame(RawFrame {
            width: crop.width,
            height: crop.height,
            stride: row_len,
            data,
            screen_x: crop.screen_x,
            screen_y: crop.screen_y,
        }))
    }
}
