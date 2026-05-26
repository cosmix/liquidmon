// SPDX-License-Identifier: MPL-2.0

mod app;
mod config;
mod devices;
mod equalizer;
mod liquidctl;
mod sparkline;
mod spinner;

fn main() -> cosmic::iced::Result {
    cosmic::applet::run::<app::AppModel>(())
}
