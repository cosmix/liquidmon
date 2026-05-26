// SPDX-License-Identifier: MPL-2.0

//! Animated fan / pump glyphs for the popup metric headers.
//!
//! These canvas widgets redraw the same blade and impeller geometry as the
//! static symbolic SVGs, but rotated by an angle derived from the device's
//! real RPM and an accumulating animation clock — so a faster fan visibly
//! spins faster. Rotation is purely cosmetic; angular velocity is scaled far
//! below true RPM so the motion stays legible rather than a blur.

use cosmic::Theme;
use cosmic::iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use cosmic::iced::{Point, Rectangle, Renderer, Vector, mouse};
use std::f32::consts::{FRAC_PI_2, TAU};

/// Radians of rotation per second, per RPM. Tuned so a ~2000 rpm pump turns
/// roughly once per second on screen instead of ~33×.
const RPM_TO_RAD_PER_S: f32 = TAU / 2000.0;

/// Which glyph to draw.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Fan,
    Pump,
}

pub struct Spinner {
    kind: Kind,
    rpm: u32,
    /// Accumulating animation clock in seconds.
    clock: f32,
}

impl Spinner {
    pub fn new(kind: Kind, rpm: u32, clock: f32) -> Self {
        Self { kind, rpm, clock }
    }

    fn angle(&self) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let speed = self.rpm as f32 * RPM_TO_RAD_PER_S;
        (self.clock * speed) % TAU
    }
}

/// One comma-shaped fan blade in local coordinates, scaled by `s` (the glyph
/// was authored against a 16 px viewBox). Mirrors `fan-symbolic.svg`.
fn fan_blade(s: f32) -> Path {
    Path::new(|p| {
        p.move_to(Point::new(-1.4 * s, -1.4 * s));
        p.bezier_curve_to(
            Point::new(-4.0 * s, -3.0 * s),
            Point::new(-5.5 * s, -6.0 * s),
            Point::new(-2.5 * s, -7.2 * s),
        );
        p.bezier_curve_to(
            Point::new(0.8 * s, -7.2 * s),
            Point::new(1.4 * s, -4.2 * s),
            Point::new(1.4 * s, -1.4 * s),
        );
        p.close();
    })
}

/// One pump impeller vane in local coordinates. Mirrors `pump-symbolic.svg`.
fn pump_vane(s: f32) -> Path {
    Path::new(|p| {
        p.move_to(Point::new(0.0, -2.6 * s));
        p.bezier_curve_to(
            Point::new(1.45 * s, -2.6 * s),
            Point::new(2.6 * s, -1.45 * s),
            Point::new(2.6 * s, 0.0),
        );
        p.line_to(Point::new(1.3 * s, 0.0));
        p.bezier_curve_to(
            Point::new(1.3 * s, -0.72 * s),
            Point::new(0.72 * s, -1.3 * s),
            Point::new(0.0, -1.3 * s),
        );
        p.close();
    })
}

impl<Message> canvas::Program<Message, Theme> for Spinner {
    type State = ();

    // Blade/vane index casts (small loop counters) into rotation angles.
    #[allow(clippy::cast_precision_loss)]
    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());

        let fg = theme.cosmic().background.on;
        let color = cosmic::iced::Color::from_rgb(fg.red, fg.green, fg.blue);

        let size = bounds.width.min(bounds.height);
        let s = size / 16.0;
        let center = Vector::new(bounds.width * 0.5, bounds.height * 0.5);
        let angle = self.angle();

        frame.with_save(|frame| {
            frame.translate(center);
            frame.rotate(angle);
            match self.kind {
                Kind::Fan => {
                    for i in 0..4 {
                        frame.with_save(|frame| {
                            frame.rotate(i as f32 * FRAC_PI_2);
                            frame.fill(&fan_blade(s), color);
                        });
                    }
                    frame.fill(&Path::circle(Point::ORIGIN, 1.6 * s), color);
                }
                Kind::Pump => {
                    // Static housing ring (drawn rotated, but symmetric).
                    let ring = Path::circle(Point::ORIGIN, 6.0 * s);
                    frame.stroke(
                        &ring,
                        Stroke::default().with_color(color).with_width(1.4 * s),
                    );
                    for i in 0..3 {
                        frame.with_save(|frame| {
                            frame.rotate(i as f32 * (TAU / 3.0));
                            frame.fill(&pump_vane(s), color);
                        });
                    }
                    frame.fill(&Path::circle(Point::ORIGIN, 0.7 * s), color);
                }
            }
        });

        vec![frame.into_geometry()]
    }
}
