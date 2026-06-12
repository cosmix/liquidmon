// SPDX-License-Identifier: MPL-2.0

//! An 80s graphic-equalizer / VU-meter rendering of a metric history.
//!
//! The sample window is binned into a fixed number of vertical columns; each
//! column is a stack of discrete LED segments lit from the bottom up in
//! proportion to the binned value, mapped against a caller-supplied **absolute**
//! range (`lo`..=`hi`). Segment colour follows the classic VU ramp — green in
//! the lower band, amber in the middle, red at the top — with the topmost lit
//! cell drawn as a bright "peak" cap and unlit cells left as a dim
//! theme-coloured grid.
//!
//! The range is absolute on purpose: an auto-scaled window would stretch a
//! 0.2 °C wiggle across the whole meter and paint the tallest bar red, making
//! the green/amber/red ramp meaningless. With a fixed range, red means the
//! metric is genuinely high (hot coolant / near-100 % duty), not merely the
//! largest value in the recent window.

use cosmic::Theme;
use cosmic::iced::widget::canvas::{self, Frame, Geometry, Path};
use cosmic::iced::{Color, Point, Rectangle, Renderer, Size, mouse};

/// Number of LED segments stacked in each column.
const SEGMENTS: usize = 10;
/// Target horizontal pitch per column in px; the column count is derived from
/// the canvas width so the meter stays dense on wide popups and — crucially —
/// stays *constant* regardless of how many samples have accumulated.
const COL_PITCH: f32 = 7.0;

/// Lower band (green) upper bound and middle band (amber) upper bound, as a
/// fraction of the full segment stack. Above the amber bound is red.
const GREEN_MAX: f32 = 0.6;
const AMBER_MAX: f32 = 0.85;

const GREEN: Color = Color::from_rgb(0.29, 0.84, 0.46);
const AMBER: Color = Color::from_rgb(0.98, 0.75, 0.22);
const RED: Color = Color::from_rgb(0.95, 0.33, 0.31);

pub struct Equalizer {
    samples: Vec<f64>,
    /// Absolute value mapped to an empty meter (bottom).
    lo: f64,
    /// Absolute value mapped to a full meter (top); values are clamped into
    /// `lo..=hi` before normalising.
    hi: f64,
}

impl Equalizer {
    pub fn new(samples: impl IntoIterator<Item = f64>, lo: f64, hi: f64) -> Self {
        Self {
            samples: samples.into_iter().collect(),
            lo,
            hi,
        }
    }
}

/// Average the sample window into `cols` left-to-right bins (oldest → newest).
#[allow(clippy::cast_precision_loss)]
fn bin(samples: &[f64], cols: usize) -> Vec<f64> {
    (0..cols)
        .map(|c| {
            let lo = c * samples.len() / cols;
            let hi = ((c + 1) * samples.len() / cols).max(lo + 1);
            let slice = &samples[lo..hi.min(samples.len())];
            slice.iter().sum::<f64>() / slice.len() as f64
        })
        .collect()
}

/// Colour for a segment by its position in the stack (0 = bottom).
#[allow(clippy::cast_precision_loss)]
fn segment_color(seg: usize) -> Color {
    let frac = seg as f32 / (SEGMENTS - 1) as f32;
    if frac < GREEN_MAX {
        GREEN
    } else if frac < AMBER_MAX {
        AMBER
    } else {
        RED
    }
}

impl<Message> canvas::Program<Message, Theme> for Equalizer {
    type State = ();

    // Geometry math casts sample/segment counts and values between usize/f64
    // and f32 pixel space; every cast here is a bounded screen coordinate.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        if self.samples.is_empty() {
            return vec![frame.into_geometry()];
        }

        // Column count is fixed by canvas width, NOT sample count, so the bar
        // count stays constant as history accumulates. Whatever samples exist
        // are spread across these columns by `bin`.
        let cols = (bounds.width / COL_PITCH) as usize;
        let cols = cols.max(1);
        let values = bin(&self.samples, cols);

        // Normalise against the absolute caller-supplied range so the colour
        // ramp tracks real magnitude rather than the window's own min/max.
        let span = (self.hi - self.lo).max(f64::EPSILON);

        // Dim grid colour follows the theme foreground so unlit cells read as
        // an inert LED matrix on any background.
        let fg = theme.cosmic().background.on;
        let unlit = Color::from_rgba(fg.red, fg.green, fg.blue, 0.10);

        let col_pitch = bounds.width / cols as f32;
        let col_w = (col_pitch * 0.72).max(1.0);
        let seg_pitch = bounds.height / SEGMENTS as f32;
        let seg_h = (seg_pitch * 0.74).max(1.0);

        for (c, &v) in values.iter().enumerate() {
            let x = c as f32 * col_pitch + (col_pitch - col_w) * 0.5;
            let norm = (((v - self.lo) / span) as f32).clamp(0.0, 1.0);
            // Always light at least the floor segment so every bin reads as a
            // bar rather than a gap.
            let lit = ((norm * SEGMENTS as f32).round() as usize).clamp(1, SEGMENTS);

            for seg in 0..SEGMENTS {
                // Segment 0 sits at the bottom; y grows downward.
                let y = bounds.height - (seg as f32 + 1.0) * seg_pitch + (seg_pitch - seg_h) * 0.5;
                let rect = Path::rectangle(Point::new(x, y), Size::new(col_w, seg_h));
                let color = if seg >= lit {
                    unlit
                } else if seg == lit - 1 {
                    // Peak cap: full-intensity tip of the lit stack.
                    segment_color(seg)
                } else {
                    let base = segment_color(seg);
                    Color { a: 0.82, ..base }
                };
                frame.fill(&rect, color);
            }
        }

        vec![frame.into_geometry()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn approx_f32(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    // --- bin() tests ---

    #[test]
    fn bin_fewer_samples_than_cols_still_returns_cols_values() {
        // 3 samples, 5 cols — each output bin is finite and output len == cols.
        let out = bin(&[1.0, 2.0, 3.0], 5);
        assert_eq!(out.len(), 5);
        for v in &out {
            assert!(v.is_finite(), "bin produced non-finite value: {v}");
        }
    }

    #[test]
    fn bin_samples_equal_cols_returns_same_len() {
        let samples: Vec<f64> = (1_i32..=4).map(f64::from).collect();
        let out = bin(&samples, 4);
        assert_eq!(out.len(), 4);
        for v in &out {
            assert!(v.is_finite(), "bin produced non-finite value: {v}");
        }
    }

    #[test]
    fn bin_more_samples_than_cols_returns_cols_len() {
        let samples: Vec<f64> = (1_i32..=20).map(f64::from).collect();
        let out = bin(&samples, 4);
        assert_eq!(out.len(), 4);
        for v in &out {
            assert!(v.is_finite(), "bin produced non-finite value: {v}");
        }
    }

    #[test]
    fn bin_single_sample_no_panic_or_nan() {
        let out = bin(&[42.0], 5);
        assert_eq!(out.len(), 5);
        for v in &out {
            assert!(v.is_finite(), "bin produced non-finite value: {v}");
        }
    }

    #[test]
    fn bin_cols_one_returns_mean_of_all_samples() {
        let out = bin(&[10.0, 20.0, 30.0], 1);
        assert_eq!(out.len(), 1);
        // mean of [10, 20, 30] = 20.0
        assert!(approx(out[0], 20.0), "expected 20.0, got {}", out[0]);
    }

    #[test]
    fn bin_averaging_correctness() {
        // 8 samples, 2 cols → first bin covers samples[0..4], second covers [4..8].
        let samples = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let out = bin(&samples, 2);
        assert_eq!(out.len(), 2);
        // Hand-computed: bin 0 = (1+2+3+4)/4 = 2.5, bin 1 = (5+6+7+8)/4 = 6.5
        assert!(approx(out[0], 2.5), "bin 0: expected 2.5, got {}", out[0]);
        assert!(approx(out[1], 6.5), "bin 1: expected 6.5, got {}", out[1]);
    }

    // --- segment_color() tests ---
    // SEGMENTS=10, GREEN_MAX=0.6, AMBER_MAX=0.85.
    // frac = seg / (SEGMENTS-1) = seg / 9.

    #[test]
    fn segment_color_lowest_is_green() {
        // seg=0: frac=0/9=0.0 < GREEN_MAX(0.6) → GREEN
        let c = segment_color(0);
        assert!(
            approx_f32(c.r, GREEN.r) && approx_f32(c.g, GREEN.g) && approx_f32(c.b, GREEN.b),
            "expected GREEN at seg=0, got {c:?}"
        );
    }

    #[test]
    fn segment_color_boundary_green_to_amber() {
        // seg=5: frac=5/9≈0.556 → GREEN (below GREEN_MAX=0.6)
        let c5 = segment_color(5);
        assert!(
            approx_f32(c5.r, GREEN.r),
            "seg=5 should be GREEN, got {c5:?}"
        );
        // seg=6: frac=6/9≈0.667 → AMBER (≥ GREEN_MAX, < AMBER_MAX)
        let c6 = segment_color(6);
        assert!(
            approx_f32(c6.r, AMBER.r) && approx_f32(c6.g, AMBER.g),
            "seg=6 should be AMBER, got {c6:?}"
        );
    }

    #[test]
    fn segment_color_boundary_amber_to_red() {
        // seg=7: frac=7/9≈0.778 → AMBER (< AMBER_MAX=0.85)
        let c7 = segment_color(7);
        assert!(
            approx_f32(c7.r, AMBER.r) && approx_f32(c7.g, AMBER.g),
            "seg=7 should be AMBER, got {c7:?}"
        );
        // seg=8: frac=8/9≈0.889 → RED (≥ AMBER_MAX)
        let c8 = segment_color(8);
        assert!(
            approx_f32(c8.r, RED.r) && approx_f32(c8.b, RED.b),
            "seg=8 should be RED, got {c8:?}"
        );
    }

    #[test]
    fn segment_color_topmost_is_red() {
        // seg=9 (SEGMENTS-1): frac=1.0 → RED
        let c = segment_color(SEGMENTS - 1);
        assert!(
            approx_f32(c.r, RED.r) && approx_f32(c.b, RED.b),
            "expected RED at seg={}, got {c:?}",
            SEGMENTS - 1
        );
    }
}
