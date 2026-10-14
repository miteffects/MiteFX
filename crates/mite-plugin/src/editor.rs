//! A nih-plug `Editor` that hosts a `mite_ui::View` in a Vello/wgpu window.

use std::any::Any;
use std::collections::HashMap;
use std::num::{NonZeroIsize, NonZeroU32};
use std::ptr::NonNull;
use std::sync::Arc;

use mite_core::AtomicF32;
use mite_ui::{open_parented, ParamHost, ThemeKind, View, WindowOptions};
use nih_plug::prelude::{Editor, GuiContext, ParamPtr, Params, ParentWindowHandle};
use nih_plug::{nih_error, nih_log};
use raw_window_handle::{
    AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
    XcbWindowHandle,
};

type ViewFactory = dyn Fn() -> Box<dyn View> + Send + Sync;

/// Creates the editor for a plugin. `accent` is the plugin's key in tokens.json `color.accent`.
/// `make_view` runs each time the host opens the window.
pub fn create_editor<V, F>(
    params: Arc<dyn Params>,
    title: &'static str,
    accent: &'static str,
    make_view: F,
) -> Option<Box<dyn Editor>>
where
    V: View,
    F: Fn() -> V + Send + Sync + 'static,
{
    let probe = make_view();
    let (w, h) = probe.logical_size();
    let (w, h) = (w * probe.zoom(), h * probe.zoom());
    Some(Box::new(MiteEditor {
        params,
        title,
        accent,
        logical_size: (w.round() as u32, h.round() as u32),
        make_view: Arc::new(move || Box::new(make_view()) as Box<dyn View>),
        scale: AtomicF32::new(0.0),
    }))
}

struct MiteEditor {
    params: Arc<dyn Params>,
    title: &'static str,
    accent: &'static str,
    logical_size: (u32, u32),
    make_view: Arc<ViewFactory>,
    /// Host-provided DPI scale, 0 until the host sets one.
    scale: AtomicF32,
}

impl Editor for MiteEditor {
    fn spawn(&self, parent: ParentWindowHandle, context: Arc<dyn GuiContext>) -> Box<dyn Any + Send> {
        let host = NihParamHost::new(self.params.clone(), context);
        let scale = self.scale.load();
        let options = WindowOptions {
            title: self.title.to_owned(),
            logical_size: (f64::from(self.logical_size.0), f64::from(self.logical_size.1)),
            fallback_scale: (scale > 0.0).then_some(f64::from(scale)),
            // Theme comes from the global settings once they exist (interaction.md, Settings).
            theme: ThemeKind::Dark,
            accent: self.accent,
        };
        match open_parented(&HostParent(parent), options, (self.make_view)(), Box::new(host)) {
            Ok(window) => Box::new(EditorHandle { _window: window }),
            Err(e) => {
                nih_error!("MiteFX: could not open the editor window: {e:?}");
                Box::new(())
            }
        }
    }

    fn size(&self) -> (u32, u32) {
        self.logical_size
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        nih_log!("MiteFX: host scale factor {factor}");
        self.scale.store(factor);
        true
    }

    // The view polls parameter values on its UI timer, so nothing to do here.
    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}

/// Keeps the window open; dropping it closes the window. baseview windows are `!Send` because
/// they must be closed from the thread that opened them. nih-plug creates and drops editor
/// handles on the host's GUI thread, which is that thread.
struct EditorHandle {
    _window: baseview::Window,
}
// SAFETY: see above — the handle is created and dropped on the host GUI thread.
unsafe impl Send for EditorHandle {}

/// nih-plug's parent handle speaks raw-window-handle 0.5; baseview and wgpu speak 0.6.
struct HostParent(ParentWindowHandle);

impl HasWindowHandle for HostParent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = match self.0 {
            ParentWindowHandle::X11Window(id) => {
                RawWindowHandle::Xcb(XcbWindowHandle::new(NonZeroU32::new(id).ok_or(HandleError::Unavailable)?))
            }
            ParentWindowHandle::AppKitNsView(view) => {
                RawWindowHandle::AppKit(AppKitWindowHandle::new(NonNull::new(view).ok_or(HandleError::Unavailable)?))
            }
            ParentWindowHandle::Win32Hwnd(hwnd) => RawWindowHandle::Win32(Win32WindowHandle::new(
                NonZeroIsize::new(hwnd as isize).ok_or(HandleError::Unavailable)?,
            )),
        };
        // SAFETY: the host keeps the parent window alive for as long as the editor exists.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

/// `ParamHost` over nih-plug's parameter pointers and GUI context.
///
/// Values the editor sets are shown immediately. nih-plug applies a GUI change only when the
/// event is handed to the host — at the end of the next audio block, or on a host flush when
/// audio is stopped — so reading the parameter back right after setting it returns the old
/// value. Drawing that made every drag follow the host's processing cadence instead of the mouse
/// (stepped, laggy, only inside a host). `pending` holds what the editor set until the host's
/// value catches up.
struct NihParamHost {
    /// Keeps the parameter objects that the `ParamPtr`s point into alive.
    _params: Arc<dyn Params>,
    ptrs: HashMap<String, ParamPtr>,
    context: Arc<dyn GuiContext>,
    pending: std::cell::RefCell<HashMap<String, (f32, std::time::Instant)>>,
}

/// How long a set value overrides the host's value if the host never confirms it (e.g. the
/// host rejected it or automation took over).
const PENDING_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

impl NihParamHost {
    fn new(params: Arc<dyn Params>, context: Arc<dyn GuiContext>) -> Self {
        let ptrs = params.param_map().into_iter().map(|(id, ptr, _group)| (id, ptr)).collect();
        Self { _params: params, ptrs, context, pending: std::cell::RefCell::new(HashMap::new()) }
    }

    fn ptr(&self, id: &str) -> Option<ParamPtr> {
        let ptr = self.ptrs.get(id).copied();
        if ptr.is_none() {
            nih_error!("MiteFX: UI asked for unknown parameter {id:?}");
        }
        ptr
    }
}

// SAFETY (all `unsafe` blocks below): every `ParamPtr` comes from `param_map()` of `_params`,
// which this struct keeps alive, and nih-plug's `GuiContext` methods are designed to be called
// from the editor with such pointers.
impl ParamHost for NihParamHost {
    fn normalized(&self, id: &str) -> f32 {
        let host = self.ptr(id).map_or(0.0, |p| unsafe { p.unmodulated_normalized_value() });
        let mut pending = self.pending.borrow_mut();
        match pending.get(id) {
            Some(&(value, at)) => {
                if (value - host).abs() < 1e-6 || at.elapsed() > PENDING_TIMEOUT {
                    pending.remove(id);
                    host
                } else {
                    value
                }
            }
            None => host,
        }
    }

    fn plain(&self, id: &str, normalized: f32) -> f32 {
        self.ptr(id).map_or(0.0, |p| unsafe { p.preview_plain(normalized) })
    }

    fn normalize(&self, id: &str, plain: f32) -> f32 {
        self.ptr(id).map_or(0.0, |p| unsafe { p.preview_normalized(plain) })
    }

    fn default_normalized(&self, id: &str) -> f32 {
        self.ptr(id).map_or(0.0, |p| unsafe { p.default_normalized_value() })
    }

    fn begin_gesture(&self, id: &str) {
        if let Some(p) = self.ptr(id) {
            unsafe { self.context.raw_begin_set_parameter(p) }
        }
    }

    fn set_normalized(&self, id: &str, normalized: f32) {
        if let Some(p) = self.ptr(id) {
            // Snap through the parameter's own mapping (steps, ranges) so the shown value is
            // exactly what the host will end up with.
            let normalized = unsafe { p.preview_normalized(p.preview_plain(normalized)) };
            self.pending.borrow_mut().insert(id.to_owned(), (normalized, std::time::Instant::now()));
            unsafe { self.context.raw_set_parameter_normalized(p, normalized) }
        }
    }

    fn end_gesture(&self, id: &str) {
        if let Some(p) = self.ptr(id) {
            unsafe { self.context.raw_end_set_parameter(p) }
        }
    }
}
