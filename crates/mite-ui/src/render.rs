//! Offscreen rendering: draws a [`View`] into RGBA8 pixels without a window.
//!
//! Used for design-review snapshots and for the "UI renders a frame" quality gate
//! (BLUEPRINT §10).

use std::num::NonZeroUsize;

use vello::kurbo::Affine;
use vello::util::RenderContext;
use vello::wgpu;
use vello::{AaSupport, Renderer, RendererOptions, Scene};

use crate::effects::{Compositor, Effects};
use crate::host::ParamHost;
use crate::present::Offscreen;
use crate::text::Typesetter;
use crate::tokens::Theme;
use crate::view::{DrawCx, View};

/// An RGBA8 (sRGB-encoded, unpremultiplied over an opaque background) image.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Renders one frame of `view` at `scale` × its logical size.
pub fn render_view(view: &mut dyn View, host: &dyn ParamHost, theme: &Theme, scale: f64) -> Result<Image, String> {
    let (lw, lh) = view.logical_size();
    let zoom = view.zoom();
    let scale = scale * zoom;
    let (width, height) = ((lw * scale).round() as u32, (lh * scale).round() as u32);

    let mut logical = Scene::new();
    let mut text = Typesetter::new();
    let mut fx = Effects::default();
    view.draw(&mut DrawCx { scene: &mut logical, theme, text: &mut text, host, size: (lw, lh), fx: &mut fx });

    let mut cx = RenderContext::new();
    let dev_id = pollster::block_on(cx.device(None)).ok_or("no compatible GPU device")?;
    let device = &cx.devices[dev_id].device;
    let queue = &cx.devices[dev_id].queue;
    let mut renderer = Renderer::new(
        device,
        RendererOptions {
            use_cpu: false,
            antialiasing_support: AaSupport::area_only(),
            num_init_threads: NonZeroUsize::new(1),
            pipeline_cache: None,
        },
    )
    .map_err(|e| e.to_string())?;
    let frame = Offscreen::new(device, width, height);
    let mut compositor = Compositor::new(device);
    compositor.render(device, queue, &mut renderer, &mut fx, &mut logical, Affine::scale(scale), theme, &frame, &|_| {})?;
    let texture = &frame.texture;

    let padded_row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mite-ui readback"),
        size: u64::from(padded_row * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("readback") });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded_row), rows_per_image: None },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
        .map_err(|e| e.to_string())?;
    rx.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;

    let mapped = slice.get_mapped_range().map_err(|e| e.to_string())?;
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = (row * padded_row) as usize;
        rgba.extend_from_slice(&mapped[start..start + (width * 4) as usize]);
    }
    Ok(Image { width, height, rgba })
}
