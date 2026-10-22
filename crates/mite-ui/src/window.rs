//! The baseview window that hosts a [`View`], rendering with Vello on wgpu.
//!
//! Vello draws into an offscreen storage texture; a [`Presenter`] puts it on screen (see
//! `present.rs` for the GPU and CPU paths).

use std::cell::{Cell, RefCell};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use baseview::dpi::LogicalSize;
use baseview::{
    Event, EventStatus, HandlerError, MouseCursor, MouseEvent, ScrollDelta, TimerHandle, Window, WindowContext,
    WindowHandler, WindowSettings, WindowSize,
};
use raw_window_handle::HasWindowHandle;
use vello::kurbo::{Affine, BezPath, Point, Stroke};
use vello::util::RenderContext;
use vello::{AaSupport, Renderer, RendererOptions, Scene};

use crate::host::ParamHost;
use crate::effects::{Compositor, Effects};
use crate::present::{Offscreen, PresentMode, Presenter};
use crate::text::Typesetter;
use crate::tokens::{Theme, ThemeKind};
use crate::view::{DrawCx, Key, Modifiers, MouseButton, UiEvent, View};

/// UI tick. Cheap when nothing is due; it only decides whether an animation frame is due, so
/// animation frames land within one tick of the display's rhythm. Input renders immediately.
const TICK: Duration = Duration::from_millis(4);
/// Animation period when the display refresh rate is unknown (60 Hz).
const DEFAULT_FRAME: Duration = Duration::from_micros(16_667);
const DOUBLE_CLICK_TIME: Duration = Duration::from_millis(400);
const DOUBLE_CLICK_SLOP: f64 = 4.0;
/// Pixels per wheel "line" when the platform reports pixel deltas.
const PIXELS_PER_LINE: f32 = 40.0;
/// Resize grip: the square of this size (window logical px) in the bottom-right corner.
const GRIP: f64 = 16.0;
/// Window scale range relative to the view's design size (BLUEPRINT: 50–400 %).
pub const MIN_ZOOM: f64 = 0.5;
pub const MAX_ZOOM: f64 = 4.0;

/// Asks the host to resize the editor to a logical size; returns whether it agreed. The window
/// is resized only if it did.
pub type ResizeRequest = Arc<dyn Fn(u32, u32) -> bool + Send + Sync>;

/// A size the host gave the editor (it resized its window), packed as `width << 32 | height` in
/// logical px; 0 = none pending. The window applies it on its next tick, on its own thread.
pub type HostResize = Arc<std::sync::atomic::AtomicU64>;

/// Packs a size for [`HostResize`].
pub fn pack_size(width: u32, height: u32) -> u64 {
    (u64::from(width) << 32) | u64::from(height)
}

#[derive(Clone)]
pub struct WindowOptions {
    pub title: String,
    pub logical_size: (f64, f64),
    /// Scale to use if the platform does not report one (the host's scale on Linux/Windows).
    pub fallback_scale: Option<f64>,
    pub theme: ThemeKind,
    /// Plugin key into `color.accent` in tokens.json (`eq`, `comp`, … `utility`).
    pub accent: &'static str,
    /// Embedded editors: how to ask the host for a new size. `None` (standalone windows)
    /// resizes directly.
    pub on_resize: Option<ResizeRequest>,
    /// Embedded editors: where the host's resizes arrive.
    pub host_resize: Option<HostResize>,
}

/// Opens the view as a child of a host-provided parent window.
pub fn open_parented(
    parent: &impl HasWindowHandle,
    options: WindowOptions,
    view: Box<dyn View>,
    host: Box<dyn ParamHost + Send>,
) -> Result<Window, baseview::Error> {
    let settings = settings(&options).with_parent(parent);
    let theme = Theme::load(options.theme, options.accent);
    let (on_resize, host_resize) = (options.on_resize.clone(), options.host_resize.clone());
    let window = Window::create(settings, move |ctx| UiWindow::new(ctx, view, host, theme, on_resize, host_resize))?;
    trace("created parented");
    // baseview creates windows unmapped; nothing draws until it is shown.
    window.show()?;
    trace("shown");
    Ok(window)
}

/// Opens the view in its own top-level window and blocks until it is closed. For previews.
pub fn run_standalone(
    options: WindowOptions,
    view: Box<dyn View>,
    host: Box<dyn ParamHost + Send>,
) -> Result<(), baseview::Error> {
    let theme = Theme::load(options.theme, options.accent);
    let (on_resize, host_resize) = (options.on_resize.clone(), options.host_resize.clone());
    let window = Window::create(settings(&options), move |ctx| UiWindow::new(ctx, view, host, theme, on_resize, host_resize))?;
    window.show()?;
    window.run_until_closed()
}

fn settings(options: &WindowOptions) -> WindowSettings {
    let (w, h) = options.logical_size;
    WindowSettings::new()
        .with_title(options.title.clone())
        .with_size(LogicalSize::new(w, h))
        .with_resizable(false)
        .with_fallback_scale_factor(options.fallback_scale)
}

struct Gpu {
    cx: RenderContext,
    presenter: Presenter,
    frame: Offscreen,
    renderer: Renderer,
    compositor: Compositor,
}

struct State {
    view: Box<dyn View>,
    host: Box<dyn ParamHost + Send>,
    theme: Theme,
    text: Typesetter,
    logical: Scene,
    fx: Effects,
    scale: f64,
    logical_size: (f64, f64),
    last_click: Option<(Instant, Point)>,
    /// baseview reports the position only on `CursorMoved`; buttons and wheel use the last one.
    cursor: Point,
}

struct UiWindow {
    gpu: RefCell<Gpu>,
    state: RefCell<State>,
    /// GTK host window to nudge after each frame (Linux; see host_sync.rs). The host sets its
    /// frame-sync property only once its window is mapped, so the lookup retries for a while.
    #[cfg(target_os = "linux")]
    host_sync: RefCell<HostSyncLookup>,
    /// Something changed since the last presented frame.
    ///
    /// Frames are rendered from our own UI timer, not from baseview's redraw requests: on X11
    /// baseview paces redraws with Present-extension vblank notifications, and inside Reaper
    /// under XWayland those never arrive for an embedded window, so it never drew at all.
    dirty: Cell<bool>,
    /// Animation (meters, spectrum) runs once per display refresh: `frame_period` apart,
    /// scheduled on absolute times so frames don't drift or bunch up.
    frame_period: Duration,
    next_animation: Cell<Instant>,
    _timer: Option<TimerHandle>,
    ctx: WindowContext,
    on_resize: Option<ResizeRequest>,
    host_resize: Option<HostResize>,
    /// The view's design size; the window keeps its aspect ratio and the zoom is window width
    /// over design width.
    design: (f64, f64),
    /// Grip drag in progress: pointer start (window logical px) and the window width then.
    resize_drag: Cell<Option<(Point, f64)>>,
    /// Last size asked for, so a drag doesn't repeat requests for the same pixel size.
    requested: Cell<(u32, u32)>,
    over_grip: Cell<bool>,
    /// Last pointer position in window logical px.
    pointer: Cell<Point>,
}

fn handler_error(e: impl std::error::Error + 'static) -> HandlerError {
    HandlerError::from_boxed(Box::new(e))
}

fn message_error(e: String) -> HandlerError {
    HandlerError::from_boxed(e.into())
}

impl UiWindow {
    fn new(
        ctx: WindowContext,
        view: Box<dyn View>,
        host: Box<dyn ParamHost + Send>,
        theme: Theme,
        on_resize: Option<ResizeRequest>,
        host_resize: Option<HostResize>,
    ) -> Result<Self, HandlerError> {
        let design = view.logical_size();
        let size = ctx.size();
        let (w, h) = (size.physical.width.max(1), size.physical.height.max(1));
        trace("init start");
        let mut cx = RenderContext::new();
        let mut mode = PresentMode::from_env_or_default();
        let (presenter, dev_id) = match Presenter::new(mode, &mut cx, ctx.platform_handle(), w, h) {
            Ok(p) => p,
            Err(e) if mode == PresentMode::Gpu => {
                // No usable GPU surface for this window (driver, remote desktop, …): copy frames
                // through the CPU instead of showing nothing.
                trace(&format!("gpu surface failed ({e}), falling back to cpu presentation"));
                mode = PresentMode::Cpu;
                Presenter::new(mode, &mut cx, ctx.platform_handle(), w, h).map_err(message_error)?
            }
            Err(e) => return Err(message_error(e)),
        };
        let device = &cx.devices[dev_id].device;
        let frame = Offscreen::new(device, w, h);
        let renderer = Renderer::new(
            device,
            RendererOptions {
                use_cpu: false,
                antialiasing_support: AaSupport::area_only(),
                num_init_threads: NonZeroUsize::new(1),
                pipeline_cache: None,
            },
        )
        .map_err(handler_error)?;
        let compositor = Compositor::new(device);
        trace(&format!("init done {size:?} present {mode:?} adapter {:?}", cx.devices[dev_id].adapter().get_info().name));
        let timer = ctx.create_timer(TICK).ok();
        let state = State {
            view,
            host,
            theme,
            text: Typesetter::new(),
            logical: Scene::new(),
            fx: Effects::default(),
            scale: size.scale_factor,
            logical_size: (size.logical.width, size.logical.height),
            last_click: None,
            cursor: Point::ZERO,
        };
        Ok(Self {
            #[cfg(target_os = "linux")]
            host_sync: RefCell::new(HostSyncLookup { found: None, handle: ctx.platform_handle(), attempts: 0, last: None }),
            gpu: RefCell::new(Gpu { cx, presenter, frame, renderer, compositor }),
            state: RefCell::new(state),
            dirty: Cell::new(true),
            frame_period: animation_period(),
            next_animation: Cell::new(Instant::now()),
            _timer: timer,
            requested: Cell::new((size.logical.width.round() as u32, size.logical.height.round() as u32)),
            ctx,
            on_resize,
            host_resize,
            design,
            resize_drag: Cell::new(None),
            over_grip: Cell::new(false),
            pointer: Cell::new(Point::ZERO),
        })
    }

    fn dispatch(&self, event: UiEvent) -> EventStatus {
        let mut st = self.state.borrow_mut();
        let State { view, host, .. } = &mut *st;
        let changed = view.event(&event, host.as_ref());
        drop(st);
        // Input renders right away instead of waiting for the next tick (~4 ms average).
        if changed {
            self.dirty.set(true);
            if let Err(e) = self.render() {
                trace(&format!("render failed: {e}"));
            }
        }
        EventStatus::Captured
    }
}

impl UiWindow {
    /// A key press: the view's if it uses it, otherwise the host's.
    fn key_event(&self, event: &keyboard_types::KeyboardEvent) -> EventStatus {
        if event.state != keyboard_types::KeyState::Down {
            return EventStatus::Ignored;
        }
        let key = match &event.key {
            keyboard_types::Key::Named(keyboard_types::NamedKey::Escape) => Key::Escape,
            keyboard_types::Key::Named(keyboard_types::NamedKey::Delete) => Key::Delete,
            keyboard_types::Key::Named(keyboard_types::NamedKey::Backspace) => Key::Backspace,
            keyboard_types::Key::Named(keyboard_types::NamedKey::Enter) => Key::Enter,
            keyboard_types::Key::Character(s) => match s.chars().next() {
                Some(c) => Key::Character(c),
                None => return EventStatus::Ignored,
            },
            _ => return EventStatus::Ignored,
        };
        let mut st = self.state.borrow_mut();
        let State { view, host, .. } = &mut *st;
        let used = view.event(&UiEvent::KeyDown { key, mods: mods(event.modifiers) }, host.as_ref());
        drop(st);
        if !used {
            return EventStatus::Ignored;
        }
        self.dirty.set(true);
        if let Err(e) = self.render() {
            trace(&format!("render failed: {e}"));
        }
        EventStatus::Captured
    }

    /// The corner resize grip. Returns `Some` when it consumed the event.
    fn grip_event(&self, mouse: &MouseEvent, dpi: f64, window: (f64, f64)) -> Option<EventStatus> {
        match *mouse {
            MouseEvent::CursorMoved { position, .. } => {
                let p = Point::new(position.x / dpi, position.y / dpi);
                if let Some((start, start_w)) = self.resize_drag.get() {
                    // Follow whichever axis moved further, keeping the aspect ratio.
                    let aspect = self.design.0 / self.design.1;
                    let (dx, dy) = (p.x - start.x, (p.y - start.y) * aspect);
                    let grow = if dx.abs() >= dy.abs() { dx } else { dy };
                    let zoom = ((start_w + grow) / self.design.0).clamp(MIN_ZOOM, MAX_ZOOM);
                    self.resize_to(zoom);
                    return Some(EventStatus::Captured);
                }
                let over = p.x >= window.0 - GRIP && p.y >= window.1 - GRIP;
                if over != self.over_grip.get() {
                    self.over_grip.set(over);
                    self.ctx.set_mouse_cursor(if over { MouseCursor::NwseResize } else { MouseCursor::Default }).ok();
                    self.dirty.set(true);
                }
                if over {
                    return Some(EventStatus::Captured);
                }
                None
            }
            MouseEvent::ButtonPressed { button: baseview::MouseButton::Left, .. } if self.over_grip.get() => {
                // Start from the content's width (the window may be letterboxed).
                let (zoom, _) = self.layout(window);
                self.resize_drag.set(Some((self.pointer.get(), self.design.0 * zoom)));
                Some(EventStatus::Captured)
            }
            MouseEvent::ButtonReleased { button: baseview::MouseButton::Left, .. } if self.resize_drag.get().is_some() => {
                self.resize_drag.set(None);
                self.dirty.set(true);
                Some(EventStatus::Captured)
            }
            _ => None,
        }
    }

    /// How the design is placed in a window of `window` logical px: the largest zoom that fits
    /// (within 50–400 %), centred. The host may give us any size (it can keep growing the window
    /// past 400 % during a drag), so the content letterboxes rather than the window refusing.
    fn layout(&self, window: (f64, f64)) -> (f64, Point) {
        let zoom = (window.0 / self.design.0).min(window.1 / self.design.1).clamp(MIN_ZOOM, MAX_ZOOM);
        let offset = Point::new(
            ((window.0 - self.design.0 * zoom) / 2.0).max(0.0).round(),
            ((window.1 - self.design.1 * zoom) / 2.0).max(0.0).round(),
        );
        (zoom, offset)
    }

    /// Resizes the window to `zoom` × the design size, asking the host first when embedded.
    fn resize_to(&self, zoom: f64) {
        let size = ((self.design.0 * zoom).round() as u32, (self.design.1 * zoom).round() as u32);
        if size == self.requested.get() {
            return;
        }
        let accepted = self.on_resize.as_ref().is_none_or(|ask| ask(size.0, size.1));
        trace(&format!("resize to {}x{} ({:.0} %): host {}", size.0, size.1, zoom * 100.0, if accepted { "agreed" } else { "refused" }));
        if accepted {
            self.set_window_size(size);
        }
    }

    /// Resizes our window to `size` (logical px) and re-lays out at once.
    fn set_window_size(&self, size: (u32, u32)) {
        self.requested.set(size);
        let logical = LogicalSize::new(f64::from(size.0), f64::from(size.1));
        if self.ctx.resize(logical).is_ok() {
            // baseview reports only resizes it didn't initiate, so apply our own here.
            let scale = self.state.borrow().scale;
            if let Err(e) = self.apply_size(WindowSize::from_logical(logical, scale)) {
                trace(&format!("resize failed: {e}"));
            }
            if let Err(e) = self.render() {
                trace(&format!("render failed: {e}"));
            }
        }
    }

    /// Resizes the swapchain and offscreen frame and updates the layout to `new_size`.
    fn apply_size(&self, new_size: WindowSize) -> Result<(), HandlerError> {
        let (w, h) = (new_size.physical.width, new_size.physical.height);
        if w > 0 && h > 0 {
            let mut gpu = self.gpu.borrow_mut();
            let Gpu { cx, presenter, frame, .. } = &mut *gpu;
            presenter.resize(cx, w, h).map_err(message_error)?;
            *frame = Offscreen::new(&cx.devices[presenter.dev_id()].device, w, h);
        }
        let mut st = self.state.borrow_mut();
        st.scale = new_size.scale_factor;
        st.logical_size = (new_size.logical.width, new_size.logical.height);
        self.dirty.set(true);
        Ok(())
    }

    /// Renders and presents one frame. Clears `dirty` once the frame is on screen.
    fn render(&self) -> Result<(), HandlerError> {
        let t0 = Instant::now();
        let mut st = self.state.borrow_mut();
        let State { view, host, theme, text, logical, fx, scale, logical_size, .. } = &mut *st;
        logical.reset();
        let (zoom, offset) = self.layout(*logical_size);
        view.draw(&mut DrawCx { scene: logical, theme, text, host: host.as_ref(), size: self.design, fx });
        let transform = Affine::scale(*scale) * Affine::translate(offset.to_vec2()) * Affine::scale(zoom);
        // Resize grip: three short diagonals in the bottom-right corner, in window logical px.
        let (w, h) = *logical_size;
        let mut grip = BezPath::new();
        for k in [4.0, 8.0, 12.0] {
            grip.move_to((w - 3.0, h - 3.0 - k));
            grip.line_to((w - 3.0 - k, h - 3.0));
        }
        let color = if self.over_grip.get() || self.resize_drag.get().is_some() { theme.text_mid } else { theme.text_lo };
        let grip_scale = *scale;
        let overlay = move |scene: &mut Scene| scene.stroke(&Stroke::new(1.0), Affine::scale(grip_scale), color, None, &grip);
        let t_scene = t0.elapsed();

        let mut gpu = self.gpu.borrow_mut();
        let Gpu { cx, presenter, frame, renderer, compositor } = &mut *gpu;
        let handle = &cx.devices[presenter.dev_id()];
        compositor
            .render(&handle.device, &handle.queue, renderer, fx, logical, transform, theme, frame, &overlay)
            .map_err(message_error)?;
        if gpu_timing() {
            // Diagnostic: wait for the GPU so "render" includes its work, not just the encoding.
            let _ = handle.device.poll(vello::wgpu::PollType::Wait { submission_index: None, timeout: None });
        }
        let t_gpu = t0.elapsed();
        let shown = presenter.present(cx, frame).map_err(message_error)?;
        trace(&format!(
            "t={:.6} frame scene {:.2} ms, render {:.2} ms, present {:.2} ms",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64(),
            t_scene.as_secs_f64() * 1e3,
            (t_gpu - t_scene).as_secs_f64() * 1e3,
            (t0.elapsed() - t_gpu).as_secs_f64() * 1e3
        ));
        self.dirty.set(!shown);
        #[cfg(target_os = "linux")]
        if shown {
            self.host_sync.borrow_mut().nudge();
        }
        Ok(())
    }
}

impl WindowHandler for UiWindow {
    /// Platform-driven redraws (expose, resize, first show) still come through here.
    fn draw(&self) -> Result<(), HandlerError> {
        self.render()
    }

    fn resized(&self, new_size: WindowSize) -> Result<(), HandlerError> {
        trace(&format!("resized {new_size:?}"));
        self.apply_size(new_size)
    }

    fn on_event(&self, event: Event) -> EventStatus {
        let mouse = match event {
            Event::Mouse(mouse) => mouse,
            Event::Keyboard(key) => return self.key_event(&key),
            _ => return EventStatus::Ignored,
        };
        if tracing() {
            trace(&format!("t={:.6} event {:?}", wall_clock(), std::mem::discriminant(&mouse)));
        }
        let (dpi, window) = {
            let st = self.state.borrow();
            (st.scale, st.logical_size)
        };
        let (zoom, offset) = self.layout(window);
        // Events reach the view in design units.
        let to_logical = |p: baseview::dpi::PhysicalPosition<f64>| {
            Point::new((p.x / dpi - offset.x) / zoom, (p.y / dpi - offset.y) / zoom)
        };
        if let MouseEvent::CursorMoved { position, .. } = mouse {
            self.pointer.set(Point::new(position.x / dpi, position.y / dpi));
        }
        if let Some(status) = self.grip_event(&mouse, dpi, window) {
            return status;
        }
        match mouse {
            MouseEvent::CursorMoved { position, modifiers } => {
                let pos = to_logical(position);
                self.state.borrow_mut().cursor = pos;
                self.dispatch(UiEvent::PointerMove { pos, mods: mods(modifiers) })
            }
            MouseEvent::ButtonPressed { button, modifiers } => {
                let Some(button) = map_button(button) else { return EventStatus::Ignored };
                let pos = self.state.borrow().cursor;
                let mods = mods(modifiers);
                let double = button == MouseButton::Left && {
                    let mut st = self.state.borrow_mut();
                    let now = Instant::now();
                    let is_double = st.last_click.is_some_and(|(t, p)| {
                        now.duration_since(t) <= DOUBLE_CLICK_TIME && p.distance(pos) <= DOUBLE_CLICK_SLOP
                    });
                    st.last_click = if is_double { None } else { Some((now, pos)) };
                    is_double
                };
                if double {
                    self.dispatch(UiEvent::DoubleClick { pos, mods })
                } else {
                    self.dispatch(UiEvent::PointerDown { pos, button, mods })
                }
            }
            MouseEvent::ButtonReleased { button, modifiers } => {
                let Some(button) = map_button(button) else { return EventStatus::Ignored };
                let pos = self.state.borrow().cursor;
                self.dispatch(UiEvent::PointerUp { pos, button, mods: mods(modifiers) })
            }
            MouseEvent::WheelScrolled { delta, modifiers } => {
                let lines = match delta {
                    ScrollDelta::Lines { y, .. } => y,
                    ScrollDelta::Pixels { y, .. } => y / PIXELS_PER_LINE,
                };
                let pos = self.state.borrow().cursor;
                self.dispatch(UiEvent::Wheel { pos, lines, mods: mods(modifiers) })
            }
            MouseEvent::CursorLeft => self.dispatch(UiEvent::PointerLeft),
            _ => EventStatus::Ignored,
        }
    }

    fn on_timer(&self, _timer: &TimerHandle) {
        if let Some(packed) = self.host_resize.as_ref().map(|r| r.swap(0, std::sync::atomic::Ordering::AcqRel)) {
            if packed != 0 {
                let size = ((packed >> 32) as u32, packed as u32);
                trace(&format!("host resized to {}x{}", size.0, size.1));
                self.set_window_size(size);
            }
        }
        if tracing() {
            trace(&format!("t={:.6} tick dirty={}", wall_clock(), self.dirty.get()));
        }
        let now = Instant::now();
        let mut animate = false;
        if now >= self.next_animation.get() {
            // Next slot on the display's rhythm; if we fell far behind, resynchronise.
            let mut next = self.next_animation.get() + self.frame_period;
            if next <= now {
                next = now + self.frame_period;
            }
            self.next_animation.set(next);
            let mut st = self.state.borrow_mut();
            let State { view, host, .. } = &mut *st;
            animate = view.tick(host.as_ref());
        }
        if self.dirty.get() || animate {
            if let Err(e) = self.render() {
                trace(&format!("render failed: {e}"));
            }
        } else {
            // The CPU presenter shows frames one tick late; once nothing changes, show the last.
            let mut gpu = self.gpu.borrow_mut();
            let Gpu { cx, presenter, .. } = &mut *gpu;
            if presenter.has_pending() {
                if let Err(e) = presenter.flush(cx) {
                    trace(&format!("flush failed: {e}"));
                }
            }
        }
    }
}

/// Lazily finds the host window to nudge (see host_sync.rs): every 250 ms for the first ~5 s.
#[cfg(target_os = "linux")]
struct HostSyncLookup {
    found: Option<crate::host_sync::HostSync>,
    handle: baseview::PlatformHandle,
    attempts: u32,
    last: Option<Instant>,
}

#[cfg(target_os = "linux")]
impl HostSyncLookup {
    fn nudge(&mut self) {
        if let Some(sync) = &self.found {
            sync.nudge();
            return;
        }
        // `MITE_UI_HOST_SYNC=0` disables the nudge (diagnostics).
        let disabled = std::env::var_os("MITE_UI_HOST_SYNC").is_some_and(|v| v == "0");
        if disabled || self.attempts >= 20 || self.last.is_some_and(|t| t.elapsed() < Duration::from_millis(250)) {
            return;
        }
        self.attempts += 1;
        self.last = Some(Instant::now());
        self.found = crate::host_sync::HostSync::find(&self.handle);
        if self.found.is_some() {
            trace(&format!("host frame sync target found after {} attempt(s)", self.attempts));
        }
    }
}

fn map_button(button: baseview::MouseButton) -> Option<MouseButton> {
    match button {
        baseview::MouseButton::Left => Some(MouseButton::Left),
        baseview::MouseButton::Right => Some(MouseButton::Right),
        baseview::MouseButton::Middle => Some(MouseButton::Middle),
        _ => None,
    }
}

fn mods(m: keyboard_types::Modifiers) -> Modifiers {
    use keyboard_types::Modifiers as K;
    Modifiers { shift: m.contains(K::SHIFT), alt: m.contains(K::ALT), ctrl: m.contains(K::CONTROL), meta: m.contains(K::META) }
}

/// Frame-timing diagnostics, enabled with `MITE_UI_TRACE=1` (read once per process).
pub(crate) fn tracing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MITE_UI_TRACE").is_some())
}

/// `MITE_UI_GPU_TIME=1` (with `MITE_UI_TRACE=1`): each frame waits for the GPU, so the traced
/// render time is the whole frame's GPU work. Slows the UI; for measuring only.
fn gpu_timing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MITE_UI_GPU_TIME").is_some())
}

/// Half a display refresh (XRandR on Linux, otherwise 60 Hz). Rendering animation at twice the
/// refresh guarantees a fresh frame in every refresh interval whatever the phase between our
/// timer and the display; at exactly the refresh rate, timer jitter made some intervals get two
/// frames and the next none (measured on screen: 27 ms median gaps instead of 17).
fn animation_period() -> Duration {
    #[cfg(target_os = "linux")]
    if let Some(hz) = crate::host_sync::display_refresh_hz() {
        trace(&format!("display refresh {hz:.2} Hz"));
        return Duration::from_secs_f64(0.5 / hz);
    }
    DEFAULT_FRAME / 2
}

fn wall_clock() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

fn trace(msg: &str) {
    if tracing() {
        eprintln!("[mite-ui] {msg}");
    }
}
