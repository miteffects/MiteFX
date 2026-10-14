//! The baseview window that hosts a [`View`], rendering with Vello on wgpu.
//!
//! Vello draws into an offscreen storage texture; a [`Presenter`] puts it on screen (see
//! `present.rs` for the GPU and CPU paths).

use std::cell::{Cell, RefCell};
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use baseview::dpi::LogicalSize;
use baseview::{
    Event, EventStatus, HandlerError, MouseEvent, ScrollDelta, TimerHandle, Window, WindowContext,
    WindowHandler, WindowSettings, WindowSize,
};
use raw_window_handle::HasWindowHandle;
use vello::kurbo::{Affine, Point};
use vello::util::RenderContext;
use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene};

use crate::host::ParamHost;
use crate::present::{Offscreen, PresentMode, Presenter};
use crate::text::Typesetter;
use crate::tokens::{Theme, ThemeKind};
use crate::view::{DrawCx, Modifiers, MouseButton, UiEvent, View};

/// UI frame clock (~120 Hz). Each tick polls telemetry and renders only if something changed, so
/// idle cost stays near zero while drags and meters keep up with high-refresh displays.
const TICK: Duration = Duration::from_millis(8);
const DOUBLE_CLICK_TIME: Duration = Duration::from_millis(400);
const DOUBLE_CLICK_SLOP: f64 = 4.0;
/// Pixels per wheel "line" when the platform reports pixel deltas.
const PIXELS_PER_LINE: f32 = 40.0;

#[derive(Debug, Clone)]
pub struct WindowOptions {
    pub title: String,
    pub logical_size: (f64, f64),
    /// Scale to use if the platform does not report one (the host's scale on Linux/Windows).
    pub fallback_scale: Option<f64>,
    pub theme: ThemeKind,
    /// Plugin key into `color.accent` in tokens.json (`eq`, `comp`, … `utility`).
    pub accent: &'static str,
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
    let window = Window::create(settings, move |ctx| UiWindow::new(ctx, view, host, theme))?;
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
    let window = Window::create(settings(&options), move |ctx| UiWindow::new(ctx, view, host, theme))?;
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
}

struct State {
    view: Box<dyn View>,
    host: Box<dyn ParamHost + Send>,
    theme: Theme,
    text: Typesetter,
    logical: Scene,
    scene: Scene,
    scale: f64,
    logical_size: (f64, f64),
    last_click: Option<(Instant, Point)>,
    /// baseview reports the position only on `CursorMoved`; buttons and wheel use the last one.
    cursor: Point,
}

struct UiWindow {
    gpu: RefCell<Gpu>,
    state: RefCell<State>,
    /// Something changed since the last presented frame.
    ///
    /// Frames are rendered from our own UI timer, not from baseview's redraw requests: on X11
    /// baseview paces redraws with Present-extension vblank notifications, and inside Reaper
    /// under XWayland those never arrive for an embedded window, so it never drew at all.
    dirty: Cell<bool>,
    _timer: Option<TimerHandle>,
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
    ) -> Result<Self, HandlerError> {
        let size = ctx.size();
        let (w, h) = (size.physical.width.max(1), size.physical.height.max(1));
        trace("init start");
        let mut cx = RenderContext::new();
        let mode = PresentMode::from_env_or_default();
        let (presenter, dev_id) = Presenter::new(mode, &mut cx, ctx.platform_handle(), w, h).map_err(message_error)?;
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
        trace(&format!("init done {size:?} present {mode:?} adapter {:?}", cx.devices[dev_id].adapter().get_info().name));
        let timer = ctx.create_timer(TICK).ok();
        let state = State {
            view,
            host,
            theme,
            text: Typesetter::new(),
            logical: Scene::new(),
            scene: Scene::new(),
            scale: size.scale_factor,
            logical_size: (size.logical.width, size.logical.height),
            last_click: None,
            cursor: Point::ZERO,
        };
        Ok(Self {
            gpu: RefCell::new(Gpu { cx, presenter, frame, renderer }),
            state: RefCell::new(state),
            dirty: Cell::new(true),
            _timer: timer,
        })
    }

    fn dispatch(&self, event: UiEvent) -> EventStatus {
        let mut st = self.state.borrow_mut();
        let State { view, host, .. } = &mut *st;
        if view.event(&event, host.as_ref()) {
            self.dirty.set(true);
        }
        EventStatus::Captured
    }
}

impl UiWindow {
    /// Renders and presents one frame. Clears `dirty` once the frame is on screen.
    fn render(&self) -> Result<(), HandlerError> {
        let t0 = Instant::now();
        let mut st = self.state.borrow_mut();
        let State { view, host, theme, text, logical, scene, scale, logical_size, .. } = &mut *st;
        logical.reset();
        view.draw(&mut DrawCx { scene: logical, theme, text, host: host.as_ref(), size: *logical_size });
        scene.reset();
        scene.append(logical, Some(Affine::scale(*scale)));
        let t_scene = t0.elapsed();

        let mut gpu = self.gpu.borrow_mut();
        let Gpu { cx, presenter, frame, renderer } = &mut *gpu;
        let handle = &cx.devices[presenter.dev_id()];
        renderer
            .render_to_texture(
                &handle.device,
                &handle.queue,
                scene,
                &frame.view,
                &RenderParams {
                    base_color: theme.window,
                    width: frame.width,
                    height: frame.height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(handler_error)?;
        let t_gpu = t0.elapsed();
        let shown = presenter.present(cx, frame).map_err(message_error)?;
        trace(&format!(
            "frame scene {:.2} ms, render {:.2} ms, present {:.2} ms",
            t_scene.as_secs_f64() * 1e3,
            (t_gpu - t_scene).as_secs_f64() * 1e3,
            (t0.elapsed() - t_gpu).as_secs_f64() * 1e3
        ));
        self.dirty.set(!shown);
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

    fn on_event(&self, event: Event) -> EventStatus {
        let Event::Mouse(mouse) = event else { return EventStatus::Ignored };
        let scale = self.state.borrow().scale;
        let to_logical = |p: baseview::dpi::PhysicalPosition<f64>| Point::new(p.x / scale, p.y / scale);
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
        let mut st = self.state.borrow_mut();
        let State { view, host, .. } = &mut *st;
        if view.tick(host.as_ref()) {
            self.dirty.set(true);
        }
        drop(st);
        if self.dirty.get() {
            if let Err(e) = self.render() {
                trace(&format!("render failed: {e}"));
            }
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

/// Temporary diagnostics: set MITE_UI_TRACE=1.
fn trace(msg: &str) {
    if std::env::var_os("MITE_UI_TRACE").is_some() {
        eprintln!("[mite-ui] {msg}");
    }
}
