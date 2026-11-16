//! Pixel labeling on the graphics card.
//!
//! Labeling a 1080p frame on the processor is two million table lookups per
//! frame, on cores the game and a recorder are already using. Here the same
//! lookup runs as one draw call: a pixel shader reads each pixel of the frame
//! and fetches its label from a 256×256×256 texture that is a copy of
//! `vision::filter::flat_label_table`. No color math is repeated in the
//! shader, and both fetches address whole texels with integer coordinates, so
//! the result is `label_pixel` of every pixel, bit for bit. The tests at the
//! end of this file check that for all 16.7 million colors.
use windows::core::{s, Error, Result, PCSTR};
use windows::Win32::Foundation::{E_FAIL, HMODULE};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{
    ID3DBlob, ID3DInclude, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D_DRIVER_TYPE,
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11ClassLinkage, ID3D11DepthStencilView, ID3D11Device,
    ID3D11DeviceContext, ID3D11InputLayout, ID3D11PixelShader, ID3D11RasterizerState,
    ID3D11RenderTargetView, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11Texture3D,
    ID3D11VertexShader, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CULL_NONE, D3D11_FILL_SOLID,
    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_RASTERIZER_DESC, D3D11_SDK_VERSION,
    D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_TEXTURE3D_DESC, D3D11_USAGE,
    D3D11_USAGE_DEFAULT, D3D11_USAGE_IMMUTABLE, D3D11_USAGE_STAGING, D3D11_VIEWPORT,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8_UINT, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::IDXGIAdapter;

/// One triangle covers the target; every pixel looks its own color up.
const SHADER: &str = r"
Texture2D<float4> image : register(t0);
Texture3D<uint> table : register(t1);

float4 vs_main(uint id : SV_VertexID) : SV_Position {
    return float4(id == 1 ? 3.0 : -1.0, id == 2 ? -3.0 : 1.0, 0.0, 1.0);
}

uint ps_main(float4 position : SV_Position) : SV_Target {
    float4 color = image.Load(int3(position.xy, 0));
    // An 8-bit channel arrives as n / 255. This gives n back exactly.
    uint3 c = uint3(color.rgb * 255.0 + 0.5);
    // The table is laid out blue along x, green along y, red along z.
    return table.Load(int4(c.b, c.g, c.r, 0));
}
";

/// The label texture's format: one unsigned byte per pixel.
pub const LABEL_FORMAT: DXGI_FORMAT = DXGI_FORMAT_R8_UINT;

pub struct GpuLabeler {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    vertex_shader: ID3D11VertexShader,
    pixel_shader: ID3D11PixelShader,
    rasterizer: ID3D11RasterizerState,
    /// A private copy of the frame being labeled. The labels and the pixels
    /// handed to the processor both come from it, so they always belong to
    /// the same image.
    image: ID3D11Texture2D,
    labels: ID3D11Texture2D,
    /// The image and the label table, in the shader's register order.
    inputs: [Option<ID3D11ShaderResourceView>; 2],
    target: [Option<ID3D11RenderTargetView>; 1],
    width: u32,
    height: u32,
    /// Only used by `label_pixels`.
    readback: Option<ID3D11Texture2D>,
}

impl GpuLabeler {
    /// Set up labeling of `width` × `height` BGRA frames on `device`.
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(Error::new(E_FAIL, "Empty frame"));
        }
        let vertex_code = compile(s!("vs_main"), s!("vs_5_0"))?;
        let pixel_code = compile(s!("ps_main"), s!("ps_5_0"))?;
        let mut vertex_shader = None;
        let mut pixel_shader = None;
        let mut rasterizer = None;
        unsafe {
            device.CreateVertexShader(
                blob_bytes(&vertex_code),
                None::<&ID3D11ClassLinkage>,
                Some(&mut vertex_shader),
            )?;
            device.CreatePixelShader(
                blob_bytes(&pixel_code),
                None::<&ID3D11ClassLinkage>,
                Some(&mut pixel_shader),
            )?;
            device.CreateRasterizerState(
                &D3D11_RASTERIZER_DESC {
                    FillMode: D3D11_FILL_SOLID,
                    CullMode: D3D11_CULL_NONE,
                    DepthClipEnable: true.into(),
                    ..Default::default()
                },
                Some(&mut rasterizer),
            )?;
        }

        let table = crate::vision::filter::flat_label_table();
        let mut table_texture: Option<ID3D11Texture3D> = None;
        let mut table_view = None;
        unsafe {
            device.CreateTexture3D(
                &D3D11_TEXTURE3D_DESC {
                    Width: 256,
                    Height: 256,
                    Depth: 256,
                    MipLevels: 1,
                    Format: DXGI_FORMAT_R8_UINT,
                    Usage: D3D11_USAGE_IMMUTABLE,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                },
                Some(&D3D11_SUBRESOURCE_DATA {
                    pSysMem: table.as_ptr().cast(),
                    SysMemPitch: 256,
                    SysMemSlicePitch: 256 * 256,
                }),
                Some(&mut table_texture),
            )?;
            device.CreateShaderResourceView(
                &required(table_texture)?,
                None,
                Some(&mut table_view),
            )?;
        }

        let image = texture(
            device,
            width,
            height,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            D3D11_USAGE_DEFAULT,
            D3D11_BIND_SHADER_RESOURCE.0 as u32,
        )?;
        let labels = texture(
            device,
            width,
            height,
            LABEL_FORMAT,
            D3D11_USAGE_DEFAULT,
            D3D11_BIND_RENDER_TARGET.0 as u32,
        )?;
        let mut image_view = None;
        let mut target = None;
        unsafe {
            device.CreateShaderResourceView(&image, None, Some(&mut image_view))?;
            device.CreateRenderTargetView(&labels, None, Some(&mut target))?;
        }

        Ok(Self {
            device: device.clone(),
            context: context.clone(),
            vertex_shader: required(vertex_shader)?,
            pixel_shader: required(pixel_shader)?,
            rasterizer: required(rasterizer)?,
            image,
            labels,
            inputs: [Some(required(image_view)?), Some(required(table_view)?)],
            target: [Some(required(target)?)],
            width,
            height,
            readback: None,
        })
    }

    /// A labeler on a device of its own, for replays and tests: the graphics
    /// card if there is one, otherwise the software rasterizer.
    pub fn standalone(width: u32, height: u32) -> Result<Self> {
        Self::on_driver(D3D_DRIVER_TYPE_HARDWARE, width, height)
            .or_else(|_| Self::on_driver(D3D_DRIVER_TYPE_WARP, width, height))
    }

    fn on_driver(driver: D3D_DRIVER_TYPE, width: u32, height: u32) -> Result<Self> {
        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                None::<&IDXGIAdapter>,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
        }
        Self::new(&required(device)?, &required(context)?, width, height)
    }

    /// Queue the labeling of `frame`, a BGRA texture of this labeler's size
    /// on the same device. Nothing waits for the graphics card: afterwards
    /// `image()` and `labels()` are the frame and its labels as far as later
    /// commands on this context are concerned.
    pub fn label(&self, frame: &ID3D11Texture2D) {
        unsafe {
            self.context.CopyResource(&self.image, frame);
        }
        self.draw();
    }

    fn draw(&self) {
        let context = &self.context;
        unsafe {
            context.OMSetRenderTargets(Some(&self.target), None::<&ID3D11DepthStencilView>);
            context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: self.width as f32,
                Height: self.height as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            context.RSSetState(&self.rasterizer);
            context.IASetInputLayout(None::<&ID3D11InputLayout>);
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vertex_shader, None);
            context.PSSetShader(&self.pixel_shader, None);
            context.PSSetShaderResources(0, Some(&self.inputs));
            context.Draw(3, 0);
            // Let go of both textures: the next frame is copied into one and
            // the other is copied out of.
            context.PSSetShaderResources(0, Some(&[None, None]));
            context.OMSetRenderTargets(None, None::<&ID3D11DepthStencilView>);
        }
    }

    /// The private copy of the frame last passed to `label`.
    pub fn image(&self) -> &ID3D11Texture2D {
        &self.image
    }

    /// The labels of `image()`, in `LABEL_FORMAT`.
    pub fn labels(&self) -> &ID3D11Texture2D {
        &self.labels
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Label a BGRA image held in memory and wait for the result: one byte
    /// per pixel, rows packed. For replays and tests; live capture uses
    /// `label` and reads the result back a frame later instead of waiting.
    pub fn label_pixels(&mut self, bgra: &[u8], stride: usize) -> Result<Vec<u8>> {
        let (w, h) = (self.width as usize, self.height as usize);
        if stride < w * 4 || bgra.len() < (h - 1) * stride + w * 4 {
            return Err(Error::new(E_FAIL, "Image smaller than the labeler"));
        }
        let readback = match &self.readback {
            Some(texture) => texture.clone(),
            None => {
                let texture = staging_texture(&self.device, self.width, self.height, LABEL_FORMAT)?;
                self.readback.insert(texture).clone()
            }
        };
        unsafe {
            self.context.UpdateSubresource(
                &self.image,
                0,
                None,
                bgra.as_ptr().cast(),
                stride as u32,
                0,
            );
        }
        self.draw();
        unsafe {
            self.context.CopyResource(&readback, &self.labels);
        }
        let mut labels = Vec::new();
        read_staging(&self.context, &readback, (0, 0), (w, h), 1, &mut labels)?;
        Ok(labels)
    }
}

fn required<T>(value: Option<T>) -> Result<T> {
    value.ok_or_else(|| Error::new(E_FAIL, "Direct3D returned no object"))
}

fn compile(entry: PCSTR, target: PCSTR) -> Result<ID3DBlob> {
    let mut code = None;
    let mut messages = None;
    let compiled = unsafe {
        D3DCompile(
            SHADER.as_ptr().cast(),
            SHADER.len(),
            PCSTR::null(),
            None,
            None::<&ID3DInclude>,
            entry,
            target,
            0,
            0,
            &mut code,
            Some(&mut messages),
        )
    };
    if let Err(error) = compiled {
        let text = messages
            .as_ref()
            .map(|blob| String::from_utf8_lossy(blob_bytes(blob)).into_owned())
            .unwrap_or_default();
        return Err(Error::new(
            error.code(),
            format!("Label shader did not compile: {} {text}", error.message()),
        ));
    }
    required(code)
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    // SAFETY: the blob owns this buffer for as long as it lives.
    unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    }
}

fn texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    usage: D3D11_USAGE,
    bind: u32,
) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: usage,
        BindFlags: bind,
        CPUAccessFlags: if usage == D3D11_USAGE_STAGING {
            D3D11_CPU_ACCESS_READ.0 as u32
        } else {
            0
        },
        MiscFlags: 0,
    };
    let mut created = None;
    unsafe {
        device.CreateTexture2D(&desc, None, Some(&mut created))?;
    }
    required(created)
}

/// A texture the processor can read, to copy a frame or its labels into.
pub fn staging_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
) -> Result<ID3D11Texture2D> {
    texture(device, width, height, format, D3D11_USAGE_STAGING, 0)
}

/// Copy a rectangle out of a staging texture into `out`, rows packed. Waits
/// for the graphics card if the texture's contents are still on their way.
/// Every byte of `out` is overwritten, so a reused buffer needs no clearing.
pub fn read_staging(
    context: &ID3D11DeviceContext,
    staging: &ID3D11Texture2D,
    (x, y): (usize, usize),
    (width, height): (usize, usize),
    bytes_per_pixel: usize,
    out: &mut Vec<u8>,
) -> Result<()> {
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
    }
    let source = mapped.pData as *const u8;
    let row_pitch = mapped.RowPitch as usize;
    let row_len = width * bytes_per_pixel;
    out.resize(row_len * height, 0);
    if row_len > 0 {
        for (row, target) in out.chunks_exact_mut(row_len).enumerate() {
            let offset = (y + row) * row_pitch + x * bytes_per_pixel;
            // SAFETY: the caller keeps the rectangle inside the texture, and
            // a mapped row holds at least the texture's width in pixels.
            unsafe {
                std::ptr::copy_nonoverlapping(source.add(offset), target.as_mut_ptr(), row_len);
            }
        }
    }
    unsafe {
        context.Unmap(staging, 0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vision::filter::{label_bgra, label_pixel};

    /// A labeler on the graphics card and one on the software rasterizer,
    /// whichever of them this machine has. A build server usually has only
    /// the second; a device that cannot be created is skipped, not failed.
    fn labelers(width: u32, height: u32) -> Vec<(&'static str, GpuLabeler)> {
        [
            ("graphics card", D3D_DRIVER_TYPE_HARDWARE),
            ("software rasterizer", D3D_DRIVER_TYPE_WARP),
        ]
        .into_iter()
        .filter_map(
            |(name, driver)| match GpuLabeler::on_driver(driver, width, height) {
                Ok(labeler) => Some((name, labeler)),
                Err(error) => {
                    eprintln!("skipped: no {name} for labeling ({error})");
                    None
                }
            },
        )
        .collect()
    }

    #[test]
    fn every_color_gets_the_label_of_label_pixel() {
        // All 16.7 million colors as one 4096 × 4096 image, in rows with a
        // stride wider than the image to cover row padding as well.
        const SIDE: usize = 4096;
        let stride = SIDE * 4 + 16;
        let mut bgra = vec![0u8; stride * SIDE];
        for (y, row) in bgra.chunks_exact_mut(stride).enumerate() {
            for (x, px) in row[..SIDE * 4].chunks_exact_mut(4).enumerate() {
                let color = (y * SIDE + x) as u32;
                // Blue, green, red; alpha varies and must not matter.
                px.copy_from_slice(&[
                    color as u8,
                    (color >> 8) as u8,
                    (color >> 16) as u8,
                    (x ^ y) as u8,
                ]);
            }
        }
        let cpu = label_bgra(&bgra, stride, SIDE, SIDE);
        for (name, mut labeler) in labelers(SIDE as u32, SIDE as u32) {
            let gpu = labeler.label_pixels(&bgra, stride).expect("gpu labeling");
            assert_eq!(gpu.len(), cpu.len());
            if let Some(i) = (0..gpu.len()).find(|&i| gpu[i] != cpu[i]) {
                panic!(
                    "color r={} g={} b={}: {name} says {}, processor says {}",
                    i >> 16,
                    (i >> 8) & 255,
                    i & 255,
                    gpu[i],
                    cpu[i]
                );
            }
            // The reference above is the lookup table. Tie a sample of it to
            // the classification itself, and make sure not all was zero.
            for i in (0..gpu.len()).step_by(4099) {
                let (r, g, b) = ((i >> 16) as u8, (i >> 8) as u8, i as u8);
                assert_eq!(gpu[i], label_pixel(r, g, b), "{name}: {r},{g},{b}");
            }
            assert!(gpu.iter().filter(|&&label| label != 0).count() > 1_000_000);
            eprintln!("all 16777216 colors labeled identically on the {name}");
        }
    }

    #[test]
    fn odd_sizes_keep_every_pixel_in_place() {
        // A size that is not a multiple of anything, each pixel a different
        // color: a shifted, flipped or padded result cannot pass.
        let (w, h) = (333usize, 127usize);
        let mut bgra = vec![0u8; w * h * 4];
        for (i, px) in bgra.chunks_exact_mut(4).enumerate() {
            let color = (i as u32).wrapping_mul(2_654_435_761);
            px.copy_from_slice(&[color as u8, (color >> 8) as u8, (color >> 16) as u8, 255]);
        }
        for (name, mut labeler) in labelers(w as u32, h as u32) {
            for _ in 0..2 {
                let gpu = labeler.label_pixels(&bgra, w * 4).expect("gpu labeling");
                assert!(gpu == label_bgra(&bgra, w * 4, w, h), "{name}");
                // A second, different image through the same labeler.
                bgra.rotate_left(4 * 1000);
            }
        }
    }
}
