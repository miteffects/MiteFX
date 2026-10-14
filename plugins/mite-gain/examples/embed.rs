//! Mimics a plugin host: opens an empty parent window and embeds the Mite Gain editor in it as a
//! child window, the same way `mite-plugin` does inside a DAW.
//!
//!     cargo run -p mite_gain --example embed

use std::sync::Arc;
use std::sync::mpsc;

use baseview::dpi::LogicalSize;
use baseview::{Event, EventStatus, HandlerError, PlatformHandle, Window, WindowHandler, WindowSettings, WindowSize};
use mite_core::StereoMeter;
use mite_gain::editor::{GainView, HEIGHT, WIDTH};
use mite_gain::params::GAIN;
use mite_ui::{ThemeKind, open_parented, MemoryParamHost, WindowOptions};

struct Empty;

impl WindowHandler for Empty {
    fn draw(&self) -> Result<(), HandlerError> {
        Ok(())
    }
    fn resized(&self, _: WindowSize) -> Result<(), HandlerError> {
        Ok(())
    }
    fn on_event(&self, _: Event) -> EventStatus {
        EventStatus::Ignored
    }
}

fn main() {
    let (tx, rx) = mpsc::channel::<PlatformHandle>();
    let settings = WindowSettings::new().with_title("fake host").with_size(LogicalSize::new(WIDTH + 40.0, HEIGHT + 40.0));
    let parent = Window::create(settings, move |ctx| {
        tx.send(ctx.platform_handle()).unwrap();
        Ok(Empty)
    })
    .expect("parent window");
    eprintln!("parent created");
    let handle = rx.recv().expect("parent handle");
    eprintln!("got parent handle");

    let meter = Arc::new(StereoMeter::new(-150.0));
    meter.store(-10.0, -7.0);
    let options = WindowOptions { title: "Mite Gain".into(), logical_size: (WIDTH, HEIGHT), fallback_scale: None, theme: ThemeKind::Dark, accent: "utility", on_resize: None, host_resize: None };
    match open_parented(&handle, options, Box::new(GainView::new(meter)), Box::new(MemoryParamHost::new().with(GAIN))) {
        Ok(child) => {
            eprintln!("child window opened");
            let _keep = child;
            parent.run_until_closed().ok();
        }
        Err(e) => eprintln!("open_parented failed: {e:?}"),
    }
}
