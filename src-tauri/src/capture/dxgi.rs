use super::window_finder::WindowFinder;
use super::RawFrame;
use windows::core::{Interface, Result};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIDevice,
    IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
};

pub struct DxgiCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging_texture: Option<ID3D11Texture2D>,
    width: u32,
    height: u32,
}

impl DxgiCapture {
    pub fn new() -> Result<Self> {
        let (device, context) = Self::create_d3d_device()?;
        let (duplication, width, height) = Self::create_duplication(&device)?;

        let mut instance = Self {
            device,
            context,
            duplication,
            staging_texture: None,
            width,
            height,
        };

        instance.ensure_staging_texture(width, height)?;
        Ok(instance)
    }

    fn create_d3d_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
        let mut device = None;
        let mut context = None;
        let feature_levels = [D3D_FEATURE_LEVEL_11_0];

        // First try hardware driver
        let hr = unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        };

        if hr.is_err() {
            // Fallback to WARP software renderer
            unsafe {
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_WARP,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&feature_levels),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )?;
            }
        }

        Ok((device.unwrap(), context.unwrap()))
    }

    fn create_duplication(device: &ID3D11Device) -> Result<(IDXGIOutputDuplication, u32, u32)> {
        unsafe {
            let dxgi_device: IDXGIDevice = device.cast()?;
            let adapter = dxgi_device.GetAdapter()?;
            let output = adapter.EnumOutputs(0)?;
            let output1: IDXGIOutput1 = output.cast()?;
            let duplication = output1.DuplicateOutput(device)?;

            let desc = output.GetDesc()?;
            let width = (desc.DesktopCoordinates.right - desc.DesktopCoordinates.left).abs() as u32;
            let height = (desc.DesktopCoordinates.bottom - desc.DesktopCoordinates.top).abs() as u32;

            Ok((duplication, width, height))
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

    pub fn reinitialize(&mut self) -> Result<()> {
        let (duplication, width, height) = Self::create_duplication(&self.device)?;
        self.duplication = duplication;
        self.ensure_staging_texture(width, height)?;
        Ok(())
    }

    pub fn capture_frame(&mut self) -> Result<Option<RawFrame>> {
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;

        let hr = unsafe {
            self.duplication
                .AcquireNextFrame(16, &mut frame_info, &mut resource)
        };

        if let Err(e) = hr {
            if e.code() == DXGI_ERROR_WAIT_TIMEOUT {
                return Ok(None);
            }
            return Err(e);
        }

        let resource = match resource {
            Some(r) => r,
            None => {
                let _ = unsafe { self.duplication.ReleaseFrame() };
                return Ok(None);
            }
        };

        let desktop_texture: ID3D11Texture2D = match resource.cast() {
            Ok(t) => t,
            Err(e) => {
                let _ = unsafe { self.duplication.ReleaseFrame() };
                return Err(e);
            }
        };

        let staging = match &self.staging_texture {
            Some(s) => s.clone(),
            None => {
                let _ = unsafe { self.duplication.ReleaseFrame() };
                return Ok(None);
            }
        };

        // Copy desktop resource to CPU-readable staging texture
        unsafe {
            self.context.CopyResource(&staging, &desktop_texture);
        }

        let _ = unsafe { self.duplication.ReleaseFrame() };

        // Determine if Genshin window is running to crop capture
        let hwnd = match WindowFinder::find_genshin() {
            Some(h) => h,
            None => {
                // Genshin is not running or is minimized - pause capture
                return Ok(None);
            }
        };

        let crop = match WindowFinder::get_client_rect(hwnd) {
            Some(r) => r,
            None => {
                // Invalid window bounds or minimized
                return Ok(None);
            }
        };

        // Map staging buffer to read pixels
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        }

        let p_data = mapped.pData as *const u8;
        let row_pitch = mapped.RowPitch as usize;

        // Clip to desktop bounds
        let x0 = crop.x.max(0) as u32;
        let y0 = crop.y.max(0) as u32;
        let x1 = (crop.x + crop.width as i32).max(0) as u32;
        let y1 = (crop.y + crop.height as i32).max(0) as u32;

        let cx0 = x0.min(self.width);
        let cy0 = y0.min(self.height);
        let cx1 = x1.min(self.width);
        let cy1 = y1.min(self.height);

        let crop_w = cx1.saturating_sub(cx0);
        let crop_h = cy1.saturating_sub(cy0);

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

            Some(RawFrame {
                width: crop_w,
                height: crop_h,
                stride: (crop_w * 4) as usize,
                data,
                screen_x: cx0 as i32,
                screen_y: cy0 as i32,
            })
        } else {
            None
        };

        unsafe {
            self.context.Unmap(&staging, 0);
        }

        Ok(frame)
    }
}
