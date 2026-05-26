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
