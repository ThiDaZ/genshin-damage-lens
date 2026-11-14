use super::window_finder::{WindowFinder, WindowRect};
use super::{clip_to_output, CaptureOutcome, RawFrame};
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
    staging_texture: Option<ID3D11Texture2D>,
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
            staging_texture: None,
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
        if self.staging_texture.is_some() && self.width == width && self.height == height {
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

        let mut tex = None;
        unsafe {
            self.device.CreateTexture2D(&desc, None, Some(&mut tex))?;
        }
        self.staging_texture = tex;
        self.width = width;
        self.height = height;
        Ok(())
    }

    pub fn capture_frame(&mut self) -> Result<CaptureOutcome> {
        // Check game availability independently of whether the desktop changed.
        let crop = match WindowFinder::find_genshin().and_then(WindowFinder::get_client_rect) {
            Some(rect) => rect,
            None => return Ok(CaptureOutcome::Unavailable),
        };
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
            None => return Ok(CaptureOutcome::Unavailable),
        };
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;

        let hr = unsafe {
            self.duplication
                .AcquireNextFrame(16, &mut frame_info, &mut resource)
        };

        if let Err(e) = hr {
            if e.code() == DXGI_ERROR_WAIT_TIMEOUT {
                return Ok(CaptureOutcome::NoNewFrame);
            }
            return Err(e);
        }

        let resource = match resource {
            Some(r) => r,
            None => {
                let _ = unsafe { self.duplication.ReleaseFrame() };
                return Ok(CaptureOutcome::NoNewFrame);
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
        let staging = match &self.staging_texture {
            Some(s) => s.clone(),
            None => {
                let _ = unsafe { self.duplication.ReleaseFrame() };
                return Ok(CaptureOutcome::Unavailable);
            }
        };

        // Copy desktop resource to CPU-readable staging texture
        unsafe {
            self.context.CopyResource(&staging, &desktop_texture);
        }

        let _ = unsafe { self.duplication.ReleaseFrame() };

        // Map staging buffer to read pixels
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        }

        let p_data = mapped.pData as *const u8;
        let row_pitch = mapped.RowPitch as usize;

        let cx0 = crop.local_x;
        let cy0 = crop.local_y;
        let crop_w = crop.width;
        let crop_h = crop.height;

        let frame = if crop_w >= 640 && crop_h >= 480 {
            let mut data = vec![0u8; (crop_w * crop_h * 4) as usize];
            for row in 0..crop_h {
                let src_offset = ((cy0 + row) as usize * row_pitch) + (cx0 as usize * 4);
                let dst_offset = (row * crop_w * 4) as usize;
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        p_data.add(src_offset),
                        data.as_mut_ptr().add(dst_offset),
                        (crop_w * 4) as usize,
                    );
                }
            }

            CaptureOutcome::Frame(RawFrame {
                width: crop_w,
                height: crop_h,
                stride: (crop_w * 4) as usize,
                data,
                screen_x: crop.screen_x,
                screen_y: crop.screen_y,
            })
        } else {
            CaptureOutcome::Unavailable
        };

        unsafe {
            self.context.Unmap(&staging, 0);
        }

        Ok(frame)
    }
}
