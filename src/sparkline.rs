// SPDX-License-Identifier: MPL-2.0

//! A tiny canvas widget that draws a polyline of recent samples as a sparkline.
//!
//! The y-axis auto-scales to the visible sample window so trends and spikes
//! fill the canvas vertically, with a small minimum span so flat traces and
//! sub-degree sensor noise don't get amplified into apparent chaos.

use cosmic::Theme;
use cosmic::iced::widget::canvas::gradient::Linear;
use cosmic::iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use cosmic::iced::{Color, Point, Rectangle, Renderer, mouse};

/// Smallest y-axis span the sparkline will ever use, in the same units as
/// the samples (°C). When the actual sample range is smaller than this,
/// the band is centered on the data midpoint so the trace renders mid-canvas.
const MIN_Y_SPAN: f64 = 2.0;

pub struct Sparkline {
    samples: Vec<f64>,
    stroke_alpha: f32,
}

impl Sparkline {
    pub fn new(samples: impl IntoIterator<Item = f64>) -> Self {
        Self {
            samples: samples.into_iter().collect(),
            stroke_alpha: 0.95,
        }
    }

    #[allow(dead_code)]
    pub fn with_stroke_alpha(mut self, alpha: f32) -> Self {
        self.stroke_alpha = alpha;
        self
    }
}

/// Compute the (min, max) y-axis bounds for a sample set.
///
/// Auto-scales from the sample range so spikes fill the canvas, but enforces
/// `MIN_Y_SPAN` centered on the data midpoint when the natural range is
/// narrower — this prevents noise from looking like real movement.
fn y_range(samples: &[f64]) -> (f64, f64) {
    if samples.is_empty() {
        let half = MIN_Y_SPAN * 0.5;
        return (-half, half);
    }
    let mut min = samples[0];
    let mut max = samples[0];
    for &s in &samples[1..] {
        if s < min {
            min = s;
        }
        if s > max {
            max = s;
        }
    }
    let span = max - min;
    if span < MIN_Y_SPAN {
        let mid = (min + max) * 0.5;
        let half = MIN_Y_SPAN * 0.5;
        (mid - half, mid + half)
    } else {
        (min, max)
    }
}

/// Build a vertical linear gradient (top = opaque accent, bottom = transparent)
/// that spans the full frame height.
fn area_gradient(accent: Color, bounds: Rectangle) -> Linear {
    let top = Color { a: 0.55, ..accent };
    let bottom = Color { a: 0.0, ..accent };
    Linear::new(Point::new(0.0, 0.0), Point::new(0.0, bounds.height))
        .add_stop(0.0, top)
        .add_stop(1.0, bottom)
}

impl<Message> canvas::Program<Message, Theme> for Sparkline {
    type State = ();

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

        let pad = 1.0_f32;
        let usable_w = (bounds.width - 2.0 * pad).max(1.0);
        let usable_h = (bounds.height - 2.0 * pad).max(1.0);

        let (y_min, y_max) = y_range(&self.samples);
        let range = y_max - y_min;

        let srgba = theme.cosmic().accent.base;
        let accent = Color {
            r: srgba.red,
            g: srgba.green,
            b: srgba.blue,
            a: self.stroke_alpha,
        };
        let stroke_color = accent;
        let stroke = Stroke::default().with_color(stroke_color).with_width(1.5);
        let gradient = area_gradient(accent, bounds);

        // Single-sample case: draw a filled rectangle under the tick so the
        // sparkline is visible immediately after the first poll, instead of
        // waiting for a second reading.
        if self.samples.len() == 1 {
            #[allow(clippy::cast_possible_truncation)]
            let norm = ((self.samples[0] - y_min) / range) as f32;
            let tick_y = pad + (1.0 - norm) * usable_h;

            let area = Path::new(|p| {
                p.move_to(Point::new(pad, tick_y));
                p.line_to(Point::new(pad + usable_w, tick_y));
                p.line_to(Point::new(pad + usable_w, pad + usable_h));
                p.line_to(Point::new(pad, pad + usable_h));
                p.close();
            });
            frame.fill(&area, gradient);

            let tick = Path::new(|p| {
                p.move_to(Point::new(pad, tick_y));
                p.line_to(Point::new(pad + usable_w, tick_y));
            });
            frame.stroke(&tick, stroke);
            return vec![frame.into_geometry()];
        }

        let n = self.samples.len();
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let points: Vec<Point> = self
            .samples
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let x = pad + (i as f32 / (n - 1) as f32) * usable_w;
                let norm = ((s - y_min) / range) as f32;
                let y = pad + (1.0 - norm) * usable_h;
                Point::new(x, y)
            })
            .collect();

        // Area path: baseline-left → polyline points → baseline-right → close.
        let area = Path::new(|p| {
            p.move_to(Point::new(pad, pad + usable_h));
            for &pt in &points {
                p.line_to(pt);
            }
            p.line_to(Point::new(pad + usable_w, pad + usable_h));
            p.close();
        });

        let polyline = Path::new(|p| {
            for (i, &pt) in points.iter().enumerate() {
                if i == 0 {
                    p.move_to(pt);
                } else {
                    p.line_to(pt);
                }
            }
        });

        // Fill first so the stroke sits on top.
        frame.fill(&area, gradient);
        frame.stroke(&polyline, stroke);
        vec![frame.into_geometry()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn empty_samples_yield_default_min_span() {
        let (min, max) = y_range(&[]);
        assert!(approx(max - min, MIN_Y_SPAN), "span was {}", max - min);
    }

    #[test]
    fn single_sample_is_centered_in_min_span() {
        let (min, max) = y_range(&[30.0]);
        assert!(approx(min, 29.0), "min was {min}");
        assert!(approx(max, 31.0), "max was {max}");
    }

    #[test]
    fn flat_samples_are_centered_in_min_span() {
        let (min, max) = y_range(&[28.5, 28.5, 28.5]);
        assert!(approx(min, 27.5));
        assert!(approx(max, 29.5));
    }

    #[test]
    fn narrow_span_is_floored_and_centered() {
        // Natural span 0.5°C — well below MIN_Y_SPAN. Midpoint 30.25,
        // expanded to a 2.0°C band yields (29.25, 31.25).
        let (min, max) = y_range(&[30.0, 30.5]);
        assert!(approx(min, 29.25), "min was {min}");
        assert!(approx(max, 31.25), "max was {max}");
    }

    #[test]
    fn wide_span_is_used_as_is() {
        let (min, max) = y_range(&[25.0, 30.0, 45.0]);
        assert!(approx(min, 25.0));
        assert!(approx(max, 45.0));
    }

    #[test]
    fn out_of_old_static_range_is_no_longer_clipped() {
        // Values outside the old hardcoded [10, 40] band still bound the axis.
        let (min, max) = y_range(&[8.0, 9.0, 55.0]);
        assert!(approx(min, 8.0));
        assert!(approx(max, 55.0));
    }

    #[test]
    fn with_stroke_alpha_overrides_default() {
        let s = Sparkline::new([1.0, 2.0]).with_stroke_alpha(0.5);
        assert!((s.stroke_alpha - 0.5).abs() < 1e-9);
    }
}
