//! Parameter smoothing.

/// Linear ramp smoother. A new target restarts the ramp from the current value, so the output is
/// continuous; the ramp always takes the configured time, whatever the distance.
///
/// Each value is computed from the ramp's start rather than accumulated, so there is no drift and
/// the last step lands exactly on the target.
#[derive(Debug, Clone)]
pub struct LinearSmoother {
    current: f32,
    start: f32,
    target: f32,
    remaining: u32,
    ramp_samples: u32,
    /// Ease in and out along a smoothstep instead of a straight line (no corner at either end).
    s_curve: bool,
}

impl LinearSmoother {
    /// A smoother sitting at `value` with a zero-length ramp. Call [`Self::set_ramp`] before use.
    pub const fn new(value: f32) -> Self {
        Self {
            current: value,
            start: value,
            target: value,
            remaining: 0,
            ramp_samples: 0,
            s_curve: false,
        }
    }

    /// A smoother sitting at `value` that ramps along a smoothstep ([`Self::set_s_curve`]): for
    /// fades whose ends must not be corners (the latency dips).
    pub const fn new_s_curve(value: f32) -> Self {
        let mut s = Self::new(value);
        s.s_curve = true;
        s
    }

    /// Ramps along a smoothstep: the same time, but starting and arriving with zero slope. For a
    /// parameter whose effect is concentrated at one end of its range (a bell's Q, in log), where
    /// a straight ramp ends on a corner that is heard.
    pub fn set_s_curve(&mut self, on: bool) {
        self.s_curve = on;
    }

    /// Sets the ramp length from a sample rate and a time in milliseconds. Does not move the value.
    pub fn set_ramp(&mut self, sample_rate: f32, ramp_ms: f32) {
        self.ramp_samples = (sample_rate * ramp_ms * 0.001).round().max(0.0) as u32;
    }

    /// Jumps straight to `value` and stops any ramp in progress.
    pub fn reset(&mut self, value: f32) {
        self.current = value;
        self.start = value;
        self.target = value;
        self.remaining = 0;
    }

    /// Starts ramping towards `target`. Does nothing if `target` is already the target.
    #[inline]
    pub fn set_target(&mut self, target: f32) {
        if target == self.target {
            return;
        }
        self.target = target;
        if self.ramp_samples == 0 {
            self.current = target;
            self.remaining = 0;
        } else {
            self.start = self.current;
            self.remaining = self.ramp_samples;
        }
    }

    /// Advances one sample and returns the new value.
    #[inline]
    pub fn next_value(&mut self) -> f32 {
        if self.remaining > 0 {
            self.remaining -= 1;
            self.current = if self.remaining == 0 {
                // Assign rather than compute: `start + (target - start)` can be off by an ulp.
                self.target
            } else {
                let done = (self.ramp_samples - self.remaining) as f32 / self.ramp_samples as f32;
                let done = if self.s_curve { done * done * (3.0 - 2.0 * done) } else { done };
                self.start + (self.target - self.start) * done
            };
        }
        self.current
    }

    #[inline]
    pub fn current(&self) -> f32 {
        self.current
    }

    #[inline]
    pub fn target(&self) -> f32 {
        self.target
    }

    #[inline]
    pub fn is_smoothing(&self) -> bool {
        self.remaining > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smoother(ms: f32) -> LinearSmoother {
        let mut s = LinearSmoother::new(0.0);
        s.set_ramp(48_000.0, ms);
        s
    }

    #[test]
    fn reaches_target_exactly_after_ramp() {
        let mut s = smoother(10.0); // 480 samples
        s.set_target(1.0);
        for _ in 0..479 {
            assert!(s.next_value() < 1.0);
        }
        assert_eq!(s.next_value(), 1.0);
        assert!(!s.is_smoothing());
        assert_eq!(s.next_value(), 1.0);
    }

    #[test]
    fn ramp_is_monotonic_with_bounded_steps() {
        let mut s = smoother(5.0); // 240 samples
        s.set_target(2.0);
        let max_step = 2.0 / 240.0 + 1e-6;
        let mut prev = s.current();
        while s.is_smoothing() {
            let v = s.next_value();
            assert!(
                v >= prev && v - prev <= max_step,
                "step {} too big",
                v - prev
            );
            prev = v;
        }
    }

    #[test]
    fn retarget_mid_ramp_is_continuous() {
        let mut s = smoother(10.0);
        s.set_target(1.0);
        for _ in 0..100 {
            s.next_value();
        }
        let before = s.current();
        s.set_target(-1.0);
        let after = s.next_value();
        assert!((after - before).abs() <= 2.0 / 480.0 + 1e-6);
    }

    #[test]
    fn zero_length_ramp_jumps() {
        let mut s = smoother(0.0);
        s.set_target(0.5);
        assert_eq!(s.next_value(), 0.5);
    }

    #[test]
    fn the_s_curve_starts_and_lands_flat_in_the_same_time() {
        let mut s = smoother(10.0);
        s.set_s_curve(true);
        s.set_target(1.0);
        let v: Vec<f32> = (0..480).map(|_| s.next_value()).collect();
        assert_eq!(v[479], 1.0);
        assert!(v[0] < 1e-4 && 1.0 - v[478] < 1e-4, "flat at both ends");
        assert!((v[239] - 0.5).abs() < 0.01);
        assert!(v.windows(2).all(|w| w[1] >= w[0]));
    }
}
