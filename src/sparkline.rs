// SPDX-License-Identifier: MPL-2.0

//! A tiny canvas widget that draws a polyline of recent samples as a sparkline.

use cosmic::Theme;
use cosmic::iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use cosmic::iced::{Color, Point, Rectangle, Renderer, mouse};

pub struct Sparkline {
    samples: Vec<f64>,
}

impl Sparkline {
    pub fn new(samples: impl IntoIterator<Item = f64>) -> Self {
        Self {
            samples: samples.into_iter().collect(),
        }
    }
}

impl<Message> canvas::Program<Message, Theme> for Sparkline {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());

        if self.samples.len() < 2 {
            return vec![frame.into_geometry()];
        }

        // Fixed y-axis: a typical AIO liquid sits in the 25-40°C range, and 10°C
        // is below ambient — anything outside the band visually pins at the edge.
        const Y_MIN: f64 = 10.0;
        const Y_MAX: f64 = 40.0;
        let min = Y_MIN;
        let range = Y_MAX - Y_MIN;

        let n = self.samples.len();
        let pad = 1.0_f32;
        let usable_w = (bounds.width - 2.0 * pad).max(1.0);
        let usable_h = (bounds.height - 2.0 * pad).max(1.0);

        let path = Path::new(|p| {
            for (i, s) in self.samples.iter().enumerate() {
                let x = pad + (i as f32 / (n - 1) as f32) * usable_w;
                let norm = ((s - min) / range) as f32;
                let y = pad + (1.0 - norm) * usable_h;
                let pt = Point::new(x, y);
                if i == 0 {
                    p.move_to(pt);
                } else {
                    p.line_to(pt);
                }
            }
        });

        let stroke_color = Color::from_rgba8(180, 200, 230, 0.85);
        frame.stroke(
            &path,
            Stroke::default().with_color(stroke_color).with_width(1.5),
        );

        vec![frame.into_geometry()]
    }
}
