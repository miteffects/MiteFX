//! Getting a rendered frame onto the screen.
//!
//! Vello always renders into an offscreen storage texture. From there, two presenters:
//!
//! - **Gpu**: a wgpu surface on the window plus a `TextureBlitter`. Zero-copy, but it relies on
//!   the platform's Vulkan/Metal/DX12 window-system integration for *child* windows.
//! - **Cpu**: read the frame back and copy it into the window with softbuffer
//!   (XPutImage/SHM on X11, GDI on Windows, CoreGraphics on macOS). Costs one readback per
//!   frame (~1.7 MB at 720×600) but works in every host. On Linux this is the default: Vulkan
//!   presentation into a host's ARGB child window under XWayland shows nothing (found in Reaper,
//!   2026-10-14).
//!
//! `MITE_UI_PRESENT=gpu|cpu` overrides the default.

use std::num::NonZeroU32;

use baseview::PlatformHandle;
use vello::util::{RenderContext, RenderSurface};
use vello::wgpu::{self, CurrentSurfaceTexture};

/// A storage texture Vello can render into, plus a buffer to read it back.
pub(crate) struct Offscreen {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    buffer: wgpu::Buffer,
    pub width: u32,
    pub height: u32,
    padded_row: u32,
}

impl Offscreen {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mite-ui offscreen"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let padded_row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mite-ui readback"),
            size: u64::from(padded_row * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self { texture, view, buffer, width, height, padded_row }
    }

    /// Copies the texture back and hands each RGBA row to `row(y, bytes)`. Blocks until the GPU
    /// is done.
    pub fn read_rows(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mut row: impl FnMut(u32, &[u8]),
    ) -> Result<(), String> {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("readback") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.padded_row), rows_per_image: None },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
        queue.submit([encoder.finish()]);

        let slice = self.buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        {
            let mapped = slice.get_mapped_range().map_err(|e| e.to_string())?;
            let row_bytes = (self.width * 4) as usize;
            for y in 0..self.height {
                let start = (y * self.padded_row) as usize;
                row(y, &mapped[start..start + row_bytes]);
            }
        }
        self.buffer.unmap();
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PresentMode {
    Gpu,
    Cpu,
}

impl PresentMode {
    pub fn from_env_or_default() -> Self {
        match std::env::var("MITE_UI_PRESENT").as_deref() {
            Ok("gpu") => Self::Gpu,
            Ok("cpu") => Self::Cpu,
            _ if cfg!(target_os = "linux") => Self::Cpu,
            _ => Self::Gpu,
        }
    }
}

pub(crate) enum Presenter {
    Gpu(Box<RenderSurface<'static>>),
    Cpu {
        dev_id: usize,
        surface: softbuffer::Surface<PlatformHandle, PlatformHandle>,
    },
}

impl Presenter {
    /// Creates the presenter and makes sure `cx` has a device for it; returns the device index.
    pub fn new(mode: PresentMode, cx: &mut RenderContext, window: PlatformHandle, w: u32, h: u32) -> Result<(Self, usize), String> {
        match mode {
            PresentMode::Gpu => {
                let surface = pollster::block_on(cx.create_surface(window, w, h, wgpu::PresentMode::AutoVsync))
                    .map_err(|e| e.to_string())?;
                let dev_id = surface.dev_id;
                Ok((Self::Gpu(Box::new(surface)), dev_id))
            }
            PresentMode::Cpu => {
                let dev_id = pollster::block_on(cx.device(None)).ok_or("no compatible GPU device")?;
                let context = softbuffer::Context::new(window.clone()).map_err(|e| e.to_string())?;
                let mut surface = softbuffer::Surface::new(&context, window).map_err(|e| e.to_string())?;
                resize_soft(&mut surface, w, h)?;
                Ok((Self::Cpu { dev_id, surface }, dev_id))
            }
        }
    }

    pub fn resize(&mut self, cx: &RenderContext, w: u32, h: u32) -> Result<(), String> {
        match self {
            Self::Gpu(surface) => {
                cx.resize_surface(surface, w, h);
                Ok(())
            }
            Self::Cpu { surface, .. } => resize_soft(surface, w, h),
        }
    }

    /// Puts `frame` on screen. Returns `false` if the frame was skipped and should be retried.
    pub fn present(&mut self, cx: &RenderContext, frame: &Offscreen) -> Result<bool, String> {
        match self {
            Self::Gpu(surface) => {
                let handle = &cx.devices[surface.dev_id];
                let texture = match surface.surface.get_current_texture() {
                    CurrentSurfaceTexture::Success(t) | CurrentSurfaceTexture::Suboptimal(t) => t,
                    CurrentSurfaceTexture::Outdated | CurrentSurfaceTexture::Lost => {
                        cx.configure_surface(surface);
                        return Ok(false);
                    }
                    _ => return Ok(false),
                };
                let target = texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder =
                    handle.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("mite-ui blit") });
                surface.blitter.copy(&handle.device, &mut encoder, &frame.view, &target);
                handle.queue.submit([encoder.finish()]);
                handle.queue.present(texture);
                Ok(true)
            }
            Self::Cpu { dev_id, surface } => {
                let handle = &cx.devices[*dev_id];
                let mut buffer = surface.buffer_mut().map_err(|e| e.to_string())?;
                let width = frame.width as usize;
                frame.read_rows(&handle.device, &handle.queue, |y, rgba| {
                    let out = &mut buffer[y as usize * width..(y as usize + 1) * width];
                    for (px, src) in out.iter_mut().zip(rgba.chunks_exact(4)) {
                        // softbuffer wants 0xAARRGGBB; the alpha byte matters on ARGB visuals.
                        *px = 0xFF00_0000 | u32::from(src[0]) << 16 | u32::from(src[1]) << 8 | u32::from(src[2]);
                    }
                })?;
                buffer.present().map_err(|e| e.to_string())?;
                Ok(true)
            }
        }
    }

    pub fn dev_id(&self) -> usize {
        match self {
            Self::Gpu(surface) => surface.dev_id,
            Self::Cpu { dev_id, .. } => *dev_id,
        }
    }
}

fn resize_soft(surface: &mut softbuffer::Surface<PlatformHandle, PlatformHandle>, w: u32, h: u32) -> Result<(), String> {
    let (Some(w), Some(h)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else { return Ok(()) };
    surface.resize(w, h).map_err(|e| e.to_string())
}
