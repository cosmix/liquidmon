// SPDX-License-Identifier: MPL-2.0

mod app;
mod config;
mod devices;
mod liquidctl;
mod sparkline;

fn main() -> cosmic::iced::Result {
    cosmic::applet::run::<app::AppModel>(())
}
