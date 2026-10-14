//! Mite Gain editor: header, one big dial, the output meter and the help strip, using the
//! EQ screen's layout language (docs/design/mockups/eq-main.dc.html).

use std::sync::Arc;

use mite_core::StereoMeter;
use mite_ui::kurbo::{Affine, Line, Point, Rect, Stroke};
use mite_ui::peniko::Fill;
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::{Knob, KnobKind, LevelMeter};
use mite_ui::{Align, DrawCx, ParamHost, TextStyle, UiEvent, View};

use crate::params::GAIN;

/// Mite Gain belongs to the Utility family, so it wears the Utility accent (tokens.json).
pub const ACCENT: &str = "utility";
pub const WIDTH: f64 = 360.0;
pub const HEIGHT: f64 = 300.0;
const HEADER: f64 = 48.0;
const HELP: f64 = 28.0;

pub struct GainView {
    meter_source: Arc<StereoMeter>,
    knob: Knob,
    meter: LevelMeter,
    last_gain: f32,
}

impl GainView {
    pub fn new(meter_source: Arc<StereoMeter>) -> Self {
        let body_mid = HEADER + (HEIGHT - HEADER - HELP) / 2.0;
        Self {
            meter_source,
            knob: Knob::new(GAIN, "GAIN", KnobKind::Bipolar, Point::new(150.0, body_mid - 8.0), 120.0),
            meter: LevelMeter::new(Rect::new(296.0, HEADER + 18.0, 329.0, HEIGHT - HELP - 34.0)),
            last_gain: f32::NAN,
        }
    }

    fn draw_header(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, WIDTH, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (WIDTH, HEADER - 0.5)));

        draw_mark(cx, 14.0, 11.0, 26.0);
        draw_title(cx, 48.0, 14.0, "Gain");
    }

    fn draw_help(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let top = HEIGHT - HELP;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.help_strip, None, &Rect::new(0.0, top, WIDTH, HEIGHT));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, top + 0.5), (WIDTH, top + 0.5)));
        cx.text.draw(
            cx.scene,
            "Drag · Shift for fine · wheel nudges · double-click resets",
            14.0,
            top + 7.0,
            TextStyle::BODY.sized(11.0),
            theme.text_mid,
            Align::Left,
        );
    }
}

impl View for GainView {
    fn logical_size(&self) -> (f64, f64) {
        (WIDTH, HEIGHT)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, HEADER, WIDTH, HEIGHT - HELP));
        self.draw_header(cx);
        self.knob.draw(cx);
        self.meter.draw(cx);
        let r = self.meter.rect;
        cx.text.draw(cx.scene, "OUT", r.center().x, r.y1 + 6.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Center);
        self.draw_help(cx);
        self.last_gain = cx.host.normalized(GAIN.id);
    }

    fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        self.knob.event(event, host)
    }

    fn tick(&mut self, host: &dyn ParamHost) -> bool {
        let (l, r) = self.meter_source.load();
        let meter_moved = self.meter.set_levels(l, r);
        let gain_moved = host.normalized(GAIN.id) != self.last_gain;
        meter_moved || gain_moved
    }
}
