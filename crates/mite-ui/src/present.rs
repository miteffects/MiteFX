//! Getting a rendered frame onto the screen.
//!
//! Vello always renders into an offscreen storage texture ([`Offscreen`]). From there, two
//! presenters:
//!
//! - **Gpu** (default): a wgpu surface on the window plus a `TextureBlitter`. Zero-copy,
//!   immediate presentation with a one-frame queue, paced by the UI's own frame clock. (An earlier "Vulkan shows nothing" diagnosis
//!   was wrong: the window was never shown and baseview never redrew it.)
//! - **Cpu** (fallback when no GPU surface can be created): copy the frame into the window with
//!   softbuffer (XPutImage/SHM on X11, GDI on Windows, CoreGraphics on macOS).
//!
//! The Cpu path is pipelined so the UI thread never waits for the GPU: frame N is rendered,
//! swizzled to the window's native BGRA layout on the GPU and copied into one of two staging
//! buffers while frame N−1 (already finished) is memcpy'd to the window. Cost: one frame of
//! display latency. Measured at 2560×1520: a blocking readback took ~7 ms per frame (4.8 ms GPU
//! wait + 2.2 ms per-pixel conversion), which capped the UI near 60 fps (lower inside hosts).
//!
//! `MITE_UI_PRESENT=gpu|cpu` forces one or the other.

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use baseview::PlatformHandle;
use vello::util::{RenderContext, RenderSurface};
use vello::wgpu::{self, util::TextureBlitter, CurrentSurfaceTexture};

/// A storage texture Vello can render into.
pub(crate) struct Offscreen {
    /// Kept alive alongside its view.
    _texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
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
        Self { _texture: texture, view, width, height }
    }

}

fn padded_row_bytes(width: u32) -> u32 {
    (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
}

fn staging_buffer(device: &wgpu::Device, padded_row: u32, height: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mite-ui staging"),
        size: u64::from(padded_row * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    })
}

fn copy_to_buffer(encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture, buffer: &wgpu::Buffer, padded_row: u32, w: u32, h: u32) {
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded_row), rows_per_image: None },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PresentMode {
    Gpu,
    Cpu,
}

impl PresentMode {
    pub fn from_env_or_default() -> Self {
        match std::env::var("MITE_UI_PRESENT").as_deref() {
            Ok("cpu") => Self::Cpu,
            Ok("gpu") => Self::Gpu,
            _ => Self::Gpu,
        }
    }
}

/// One staging slot of the CPU pipeline.
struct Slot {
    buffer: wgpu::Buffer,
    mapped: Arc<AtomicBool>,
    in_flight: bool,
    /// The submission that fills this slot; waiting is scoped to it, never to newer work.
    submission: Option<wgpu::SubmissionIndex>,
}

/// GPU-side swizzle target plus two staging buffers.
pub(crate) struct Pipeline {
    bgra: wgpu::Texture,
    bgra_view: wgpu::TextureView,
    swizzle: TextureBlitter,
    slots: [Slot; 2],
    /// Slot the next frame goes into.
    next: usize,
    width: u32,
    height: u32,
    padded_row: u32,
}

impl Pipeline {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let bgra = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mite-ui bgra"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let bgra_view = bgra.create_view(&wgpu::TextureViewDescriptor::default());
        let padded_row = padded_row_bytes(width);
        let slot = || Slot {
            buffer: staging_buffer(device, padded_row, height),
            mapped: Arc::new(AtomicBool::new(false)),
            in_flight: false,
            submission: None,
        };
        Self {
            bgra,
            bgra_view,
            swizzle: TextureBlitter::new(device, wgpu::TextureFormat::Bgra8Unorm),
            slots: [slot(), slot()],
            next: 0,
            width,
            height,
            padded_row,
        }
    }
}

pub(crate) enum Presenter {
    Gpu(Box<RenderSurface<'static>>),
    Cpu {
        dev_id: usize,
        surface: softbuffer::Surface<PlatformHandle, PlatformHandle>,
        pipeline: Box<Pipeline>,
    },
}

impl Presenter {
    /// Creates the presenter and makes sure `cx` has a device for it; returns the device index.
    pub fn new(mode: PresentMode, cx: &mut RenderContext, window: PlatformHandle, w: u32, h: u32) -> Result<(Self, usize), String> {
        match mode {
            PresentMode::Gpu => {
                // FIFO (vsync) with a one-frame queue: the display paces rendering, exactly one
                // frame per refresh. Measured on screen (tools/onscreen_fps.py): pacing frames
                // with our own timer instead beat against the refresh and showed 16/24/33 ms gaps.
                // Mailbox and immediate don't pace at all.
                let mut surface = pollster::block_on(cx.create_surface(window, w, h, wgpu::PresentMode::Fifo))
                    .map_err(|e| e.to_string())?;
                surface.config.desired_maximum_frame_latency = 1;
                cx.configure_surface(&surface);
                let dev_id = surface.dev_id;
                Ok((Self::Gpu(Box::new(surface)), dev_id))
            }
            PresentMode::Cpu => {
                let dev_id = pollster::block_on(cx.device(None)).ok_or("no compatible GPU device")?;
                let context = softbuffer::Context::new(window.clone()).map_err(|e| e.to_string())?;
                let mut surface = softbuffer::Surface::new(&context, window).map_err(|e| e.to_string())?;
                resize_soft(&mut surface, w, h)?;
                let pipeline = Box::new(Pipeline::new(&cx.devices[dev_id].device, w, h));
                Ok((Self::Cpu { dev_id, surface, pipeline }, dev_id))
            }
        }
    }

    pub fn resize(&mut self, cx: &RenderContext, w: u32, h: u32) -> Result<(), String> {
        match self {
            Self::Gpu(surface) => {
                cx.resize_surface(surface, w, h);
                Ok(())
            }
            Self::Cpu { dev_id, surface, pipeline } => {
                **pipeline = Pipeline::new(&cx.devices[*dev_id].device, w, h);
                resize_soft(surface, w, h)
            }
        }
    }

    /// Puts `frame` on screen (Cpu: queues it and shows the previous one). Returns `false` if
    /// nothing could be shown and the caller should retry.
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
            Self::Cpu { dev_id, surface, pipeline } => {
                let handle = &cx.devices[*dev_id];
                let p = &mut **pipeline;
                if frame.width != p.width || frame.height != p.height {
                    return Ok(false);
                }
                // Queue this frame: swizzle to BGRA and copy into the free staging slot.
                let t0 = std::time::Instant::now();
                let slot_index = p.next;
                let mut encoder = handle.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("mite-ui readback") });
                p.swizzle.copy(&handle.device, &mut encoder, &frame.view, &p.bgra_view);
                copy_to_buffer(&mut encoder, &p.bgra, &p.slots[slot_index].buffer, p.padded_row, p.width, p.height);
                let submission = handle.queue.submit([encoder.finish()]);
                let slot = &mut p.slots[slot_index];
                slot.submission = Some(submission);
                slot.mapped.store(false, Ordering::Release);
                let flag = slot.mapped.clone();
                slot.buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                    if r.is_ok() {
                        flag.store(true, Ordering::Release);
                    }
                });
                slot.in_flight = true;
                p.next = 1 - slot_index;
                // Drive map callbacks without blocking.
                let _ = handle.device.poll(wgpu::PollType::Poll);

                // Show the previous frame if it is still queued (it was submitted a tick ago, so
                // the GPU is done with it). Never show the frame just queued: waiting for it is
                // exactly the stall this pipeline exists to avoid. It appears on the next tick,
                // via the next present or `flush`.
                let previous = 1 - slot_index;
                if p.slots[previous].in_flight {
                    Self::show_slot(handle, surface, p, previous)?;
                }
                if crate::window::tracing() {
                    eprintln!("[mite-ui] queue {:.2} ms", t0.elapsed().as_secs_f64() * 1e3);
                }
                Ok(true)
            }
        }
    }

    /// Shows the newest queued frame (Cpu path) once the GPU has finished it, without waiting.
    /// Called on ticks with nothing new to draw, so the last frame always reaches the screen.
    pub fn flush(&mut self, cx: &RenderContext) -> Result<(), String> {
        if let Self::Cpu { dev_id, surface, pipeline } = self {
            let handle = &cx.devices[*dev_id];
            let p = &mut **pipeline;
            let newest = 1 - p.next;
            let _ = handle.device.poll(wgpu::PollType::Poll);
            if p.slots[newest].in_flight && p.slots[newest].mapped.load(Ordering::Acquire) {
                Self::show_slot(handle, surface, p, newest)?;
            }
        }
        Ok(())
    }

    /// A frame has been queued but not shown yet (Cpu path).
    pub fn has_pending(&self) -> bool {
        matches!(self, Self::Cpu { pipeline, .. } if pipeline.slots.iter().any(|s| s.in_flight))
    }

    fn show_slot(
        handle: &vello::util::DeviceHandle,
        surface: &mut softbuffer::Surface<PlatformHandle, PlatformHandle>,
        p: &mut Pipeline,
        index: usize,
    ) -> Result<(), String> {
        let trace = crate::window::tracing();
        let t0 = std::time::Instant::now();
        let slot = &mut p.slots[index];
        let _ = handle.device.poll(wgpu::PollType::Poll);
        let was_ready = slot.mapped.load(Ordering::Acquire);
        if !slot.mapped.load(Ordering::Acquire) {
            // Wait for this slot's own submission only, not for frames queued after it.
            let submission_index = slot.submission.clone();
            handle
                .device
                .poll(wgpu::PollType::Wait { submission_index, timeout: None })
                .map_err(|e| e.to_string())?;
        }
        if !slot.mapped.load(Ordering::Acquire) {
            return Err("staging buffer failed to map".into());
        }
        let t_wait = t0.elapsed();
        {
            let mut buffer = surface.buffer_mut().map_err(|e| e.to_string())?;
            let t_buf = t0.elapsed();
            let mapped = slot.buffer.slice(..).get_mapped_range().map_err(|e| e.to_string())?;
            let row_px = p.width as usize;
            // BGRA8 bytes in little-endian u32s are exactly softbuffer's 0xAARRGGBB, and Vello's
            // output is opaque, so each row is a straight memcpy.
            for y in 0..p.height as usize {
                let src = &mapped[y * p.padded_row as usize..][..row_px * 4];
                let dst = &mut buffer[y * row_px..][..row_px];
                copy_bgra_row(src, dst);
            }
            drop(mapped);
            let t_copy = t0.elapsed();
            buffer.present().map_err(|e| e.to_string())?;
            if trace {
                eprintln!(
                    "[mite-ui] show ready={was_ready} wait {:.2} buffer {:.2} copy {:.2} put {:.2} ms",
                    t_wait.as_secs_f64() * 1e3,
                    (t_buf - t_wait).as_secs_f64() * 1e3,
                    (t_copy - t_buf).as_secs_f64() * 1e3,
                    (t0.elapsed() - t_copy).as_secs_f64() * 1e3
                );
            }
        }
        slot.buffer.unmap();
        slot.in_flight = false;
        Ok(())
    }

    pub fn dev_id(&self) -> usize {
        match self {
            Self::Gpu(surface) => surface.dev_id,
            Self::Cpu { dev_id, .. } => *dev_id,
        }
    }
}

#[cfg(target_endian = "little")]
fn copy_bgra_row(src: &[u8], dst: &mut [u32]) {
    // SAFETY: viewing `dst` (src.len() / 4 u32s) as bytes is always valid: u8 has no alignment
    // or validity requirements, and the lengths match.
    let bytes = unsafe { std::slice::from_raw_parts_mut(dst.as_mut_ptr().cast::<u8>(), dst.len() * 4) };
    bytes.copy_from_slice(src);
}

#[cfg(target_endian = "big")]
fn copy_bgra_row(src: &[u8], dst: &mut [u32]) {
    for (px, b) in dst.iter_mut().zip(src.chunks_exact(4)) {
        *px = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    }
}

fn resize_soft(surface: &mut softbuffer::Surface<PlatformHandle, PlatformHandle>, w: u32, h: u32) -> Result<(), String> {
    let (Some(w), Some(h)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else { return Ok(()) };
    surface.resize(w, h).map_err(|e| e.to_string())
}
