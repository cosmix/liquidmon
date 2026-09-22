// SPDX-License-Identifier: MPL-2.0

//! Canvas preview of a device-side fan curve.
//!
//! Draws the polyline the cooler was actually programmed with — held flat
//! below the first point and above the last, since the firmware does not
//! ramp outside the curve's own domain — plus a dashed marker at the current
//! coolant temperature. The axes are fixed (20-60 °C, 0-100% duty), matching
//! `equalizer.rs`'s absolute-range argument: an auto-scaled axis would make
//! two presets look identical.

use crate::control;

use cosmic::Theme;
use cosmic::iced::widget::canvas::{
    self, Frame, Geometry, LineCap, LineDash, LineJoin, Path, Stroke,
};
use cosmic::iced::{Color, Point, Rectangle, Renderer, mouse};

/// Fixed x-axis domain, in °C.
const TEMP_MIN_C: f64 = 20.0;
const TEMP_MAX_C: f64 = 60.0;
/// Fixed y-axis domain is 0..=`DUTY_MAX_PCT`, in percent duty.
const DUTY_MAX_PCT: f64 = 100.0;

/// Duty values, as a fraction of the fixed range, where a hairline guide is
/// drawn.
const GUIDE_DUTIES: [f64; 2] = [50.0, 75.0];

/// Dash pattern for the current-temperature marker: 4px on, 3px off.
const MARKER_DASH: [f32; 2] = [4.0, 3.0];
const MARKER_DOT_RADIUS: f32 = 3.0;

/// Canvas preview of a device-side fan curve: the polyline the cooler was
/// programmed with, plus a marker at the current coolant temperature.
pub struct CurvePreview {
    points: Vec<(u8, u8)>,
    liquid_temp_c: f64,
}

impl CurvePreview {
    /// `points` are the curve as the DEVICE runs it — the preset's points
    /// with liquidctl's appended `(60, 100)` failsafe already included
    /// (`control::effective_curve` produces exactly this).
    pub fn new(points: Vec<(u8, u8)>, liquid_temp_c: f64) -> Self {
        Self {
            points,
            liquid_temp_c,
        }
    }
}

/// Map a coolant temperature to an x pixel. Clamped into the fixed
/// 20-60 °C domain so an out-of-range reading still lands inside the frame.
fn x_for_temp(temp_c: f64, width: f32) -> f32 {
    let frac = (temp_c.clamp(TEMP_MIN_C, TEMP_MAX_C) - TEMP_MIN_C) / (TEMP_MAX_C - TEMP_MIN_C);
    // `frac` is clamped into [0.0, 1.0] above; scaled by a small on-screen
    // width it stays well within f32 range.
    #[allow(clippy::cast_possible_truncation)]
    let frac = frac as f32;
    frac * width
}

/// Map a fan duty to a y pixel. Clamped into the fixed 0-100% domain; 0%
/// sits at the bottom of the frame, 100% at the top.
fn y_for_duty(duty_pct: f64, height: f32) -> f32 {
    let frac = duty_pct.clamp(0.0, DUTY_MAX_PCT) / DUTY_MAX_PCT;
    // Same reasoning as `x_for_temp`: `frac` is clamped into [0.0, 1.0].
    #[allow(clippy::cast_possible_truncation)]
    let frac = frac as f32;
    height - frac * height
}

fn point_at(temp_c: f64, duty_pct: f64, width: f32, height: f32) -> Point {
    Point::new(x_for_temp(temp_c, width), y_for_duty(duty_pct, height))
}

/// The full drawn polyline as (temp_c, duty_pct) pairs: flat from the left
/// edge to the curve's first point (the firmware does not ramp below it),
/// then the curve's own points, then flat from the last point to the right
/// edge. Empty when `points` is empty.
fn polyline_vertices(points: &[(u8, u8)]) -> Vec<(f64, f64)> {
    let Some(&(_, first_duty)) = points.first() else {
        return Vec::new();
    };
    // Non-empty, checked via `first()` above, so `last()` cannot be `None`.
    let (_, last_duty) = *points.last().expect("points non-empty: checked above");

    let mut verts = Vec::with_capacity(points.len() + 2);
    verts.push((TEMP_MIN_C, f64::from(first_duty)));
    verts.extend(points.iter().map(|&(t, d)| (f64::from(t), f64::from(d))));
    verts.push((TEMP_MAX_C, f64::from(last_duty)));
    verts
}

/// Two hairline guides at `GUIDE_DUTIES`, drawn first so the curve and
/// marker sit on top of them.
fn draw_guides(frame: &mut Frame, color: Color, width: f32, height: f32) {
    let stroke = Stroke::default().with_color(color).with_width(1.0);
    for &duty in &GUIDE_DUTIES {
        let y = y_for_duty(duty, height);
        let guide = Path::new(|p| {
            p.move_to(Point::new(0.0, y));
            p.line_to(Point::new(width, y));
        });
        frame.stroke(&guide, stroke);
    }
}

fn draw_curve(frame: &mut Frame, vertices: &[(f64, f64)], color: Color, width: f32, height: f32) {
    let polyline = Path::new(|p| {
        for (i, &(t, d)) in vertices.iter().enumerate() {
            let pt = point_at(t, d, width, height);
            if i == 0 {
                p.move_to(pt);
            } else {
                p.line_to(pt);
            }
        }
    });
    frame.stroke(
        &polyline,
        Stroke::default()
            .with_color(color)
            .with_width(2.0)
            .with_line_join(LineJoin::Round)
            .with_line_cap(LineCap::Round),
    );
}

/// Dashed vertical line at the current temperature, plus a filled dot at
/// `marker_duty` — `control::curve_duty_at`'s output, the same value the
/// divergence check evaluates, so the dot cannot drift from the thing that
/// decides whether to write.
fn draw_marker(frame: &mut Frame, marker_x: f32, marker_duty: f64, color: Color, height: f32) {
    let marker_y = y_for_duty(marker_duty, height);
    let line = Path::new(|p| {
        p.move_to(Point::new(marker_x, 0.0));
        p.line_to(Point::new(marker_x, height));
    });
    frame.stroke(
        &line,
        Stroke {
            line_dash: LineDash {
                segments: &MARKER_DASH,
                offset: 0,
            },
            ..Stroke::default()
                .with_color(Color { a: 0.5, ..color })
                .with_width(1.5)
        },
    );
    let dot = Path::circle(Point::new(marker_x, marker_y), MARKER_DOT_RADIUS);
    frame.fill(&dot, color);
}

impl<Message> canvas::Program<Message, Theme> for CurvePreview {
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
        let (width, height) = (bounds.width, bounds.height);
        let cosmic_theme = theme.cosmic();

        let divider = cosmic_theme.bg_divider();
        let guide_color = Color::from_rgba(divider.red, divider.green, divider.blue, divider.alpha);
        draw_guides(&mut frame, guide_color, width, height);

        let vertices = polyline_vertices(&self.points);
        if !vertices.is_empty() {
            let accent = cosmic_theme.accent_color();
            let curve_color = Color::from_rgba(accent.red, accent.green, accent.blue, accent.alpha);
            draw_curve(&mut frame, &vertices, curve_color, width, height);

            let marker_duty = control::curve_duty_at(&self.points, self.liquid_temp_c);
            let marker_x = x_for_temp(self.liquid_temp_c, width);
            draw_marker(&mut frame, marker_x, marker_duty, curve_color, height);
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
        (a - b).abs() < 1e-4
    }

    // --- x_for_temp / y_for_duty ---

    #[test]
    fn x_for_temp_spans_full_width_at_range_ends_and_midpoint() {
        let w = 348.0_f32;
        assert!(approx_f32(x_for_temp(TEMP_MIN_C, w), 0.0));
        assert!(approx_f32(x_for_temp(TEMP_MAX_C, w), w));
        assert!(
            approx_f32(x_for_temp(40.0, w), w / 2.0),
            "40 C is the domain midpoint"
        );
    }

    #[test]
    fn y_for_duty_is_inverted_full_height_at_range_ends_and_midpoint() {
        let h = 46.0_f32;
        assert!(
            approx_f32(y_for_duty(0.0, h), h),
            "0% duty sits at the bottom"
        );
        assert!(
            approx_f32(y_for_duty(100.0, h), 0.0),
            "100% duty sits at the top"
        );
        assert!(approx_f32(y_for_duty(50.0, h), h / 2.0));
    }

    #[test]
    fn temp_below_range_clamps_to_left_edge() {
        assert!(approx_f32(x_for_temp(-10.0, 348.0), 0.0));
    }

    #[test]
    fn temp_above_range_clamps_to_right_edge() {
        assert!(approx_f32(x_for_temp(200.0, 348.0), 348.0));
    }

    // --- polyline_vertices ---

    #[test]
    fn polyline_holds_flat_left_of_first_point() {
        let points = [(30_u8, 20_u8), (45, 60), (60, 100)];
        let verts = polyline_vertices(&points);
        assert_eq!(
            verts[0],
            (TEMP_MIN_C, 20.0),
            "should hold at the first point's duty from the left edge"
        );
    }

    #[test]
    fn polyline_holds_flat_right_of_last_point() {
        let points = [(30_u8, 20_u8), (45, 60)];
        let verts = polyline_vertices(&points);
        assert_eq!(
            *verts.last().unwrap(),
            (TEMP_MAX_C, 60.0),
            "should hold at the last point's duty to the right edge"
        );
    }

    #[test]
    fn failsafe_point_is_present_in_drawn_vertices() {
        let points = [(30_u8, 20_u8), (60, 100)];
        let verts = polyline_vertices(&points);
        assert!(
            verts
                .iter()
                .any(|&v| approx(v.0, 60.0) && approx(v.1, 100.0)),
            "failsafe (60, 100) missing from {verts:?}"
        );
    }

    #[test]
    fn empty_points_yield_no_vertices() {
        assert!(polyline_vertices(&[]).is_empty());
    }

    // --- marker vs. control::curve_duty_at ---

    #[test]
    fn marker_y_matches_curve_duty_at_the_curves_own_points() {
        let points = vec![(30_u8, 20_u8), (45, 60), (60, 100)];
        for &(t, d) in &points {
            let duty = control::curve_duty_at(&points, f64::from(t));
            assert!(
                approx(duty, f64::from(d)),
                "curve_duty_at({t}) = {duty}, expected {d}"
            );
            assert!(approx_f32(
                y_for_duty(duty, 46.0),
                y_for_duty(f64::from(d), 46.0)
            ));
        }
    }

    #[test]
    fn marker_holds_flat_before_first_point() {
        let points = vec![(30_u8, 20_u8), (45, 60), (60, 100)];
        let duty = control::curve_duty_at(&points, 20.0);
        assert!(
            approx(duty, 20.0),
            "expected a flat hold at the first point's duty, got {duty}"
        );
    }
}
