// SPDX-License-Identifier: MPL-2.0

mod app;
mod config;
mod control;
mod control_view;
mod curve;
mod devices;
mod equalizer;
mod liquidctl;
mod sparkline;
mod spinner;
mod view;

fn main() -> cosmic::iced::Result {
    cosmic::applet::run::<app::AppModel>(())
}
