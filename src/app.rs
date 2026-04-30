// SPDX-License-Identifier: MPL-2.0

use crate::config::Config;
use crate::sparkline::Sparkline;
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::futures::channel::mpsc;
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::widget::canvas::Canvas;
use cosmic::iced::widget::row;
use cosmic::iced::{Alignment, Length, Limits, Subscription, window::Id};
use cosmic::prelude::*;
use cosmic::widget;
use cosmic::widget::autosize;
use futures_util::SinkExt;
use std::collections::VecDeque;
use std::sync::LazyLock;
use std::time::Duration;

static AUTOSIZE_ID: LazyLock<widget::Id> =
    LazyLock::new(|| widget::Id::new("liquidmon-applet"));

const MAX_SAMPLES: usize = 60;
const ICON_TEMP: &[u8] = include_bytes!("../resources/icons/temperature-symbolic.svg");
const ICON_SNOWFLAKE: &[u8] = include_bytes!("../resources/icons/snowflake-symbolic.svg");
const ICON_FAN: &[u8] = include_bytes!("../resources/icons/fan-symbolic.svg");
const ICON_PUMP: &[u8] = include_bytes!("../resources/icons/pump-symbolic.svg");

fn symbolic_icon(bytes: &'static [u8]) -> widget::icon::Icon {
    let mut handle = widget::icon::from_svg_bytes(bytes);
    handle.symbolic = true;
    widget::icon::icon(handle).size(14)
}

fn fan_duty_avg(fans: &[crate::liquidctl::Fan]) -> Option<u8> {
    if fans.is_empty() {
        return None;
    }
    let sum: u32 = fans.iter().map(|f| u32::from(f.duty_pct)).sum();
    Some((sum / fans.len() as u32) as u8)
}

/// The application model stores app-specific state used to describe its interface and
/// drive its logic.
#[derive(Default)]
pub struct AppModel {
    /// Application state which is managed by the COSMIC runtime.
    core: cosmic::Core,
    /// The popup id.
    popup: Option<Id>,
    /// Configuration data that persists between application runs.
    config: Config,
    /// The most recent successful liquidctl status reading.
    last_status: Option<crate::liquidctl::AioStatus>,
    /// The most recent error message, if any.
    last_error: Option<String>,
    /// Liquid temperature samples for the panel sparkline (oldest first).
    temp_history: VecDeque<f64>,
}

/// Messages emitted by the application and its widgets.
#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    PopupClosed(Id),
    UpdateConfig(Config),
    StatusTick(Result<crate::liquidctl::AioStatus, String>),
}

/// Create a COSMIC application from the app model
impl cosmic::Application for AppModel {
    /// The async executor that will be used to run your application's commands.
    type Executor = cosmic::executor::Default;

    /// Data that your application receives to its init method.
    type Flags = ();

    /// Messages which the application and its widgets will emit.
    type Message = Message;

    /// Unique identifier in RDNN (reverse domain name notation) format.
    const APP_ID: &'static str = "com.github.cosmix.LiquidMon";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    /// Initializes the application with any given flags and startup commands.
    fn init(
        core: cosmic::Core,
        _flags: Self::Flags,
    ) -> (Self, Task<cosmic::Action<Self::Message>>) {
        // Construct the app model with the runtime's core.
        let app = AppModel {
            core,
            config: cosmic_config::Config::new(Self::APP_ID, Config::VERSION)
                .map(|context| match Config::get_entry(&context) {
                    Ok(config) => config,
                    Err((_errors, config)) => config,
                })
                .unwrap_or_default(),
            ..Default::default()
        };

        (app, Task::none())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    /// Describes the interface based on the current state of the application model.
    ///
    /// The applet's button in the panel will be drawn using the main view method.
    /// This view should emit messages to toggle the applet's popup window, which will
    /// be drawn using the `view_window` method.
    fn view(&self) -> Element<'_, Self::Message> {
        let content: Element<'_, Self::Message> = match (&self.last_status, &self.last_error) {
            (Some(status), _) => {
                let temp_text = format!("{:.1}°", status.liquid_temp_c);
                let fan_text = match fan_duty_avg(&status.fans) {
                    Some(p) => format!("{p}%"),
                    None => "—".to_string(),
                };
                let pump_text = format!("{}%", status.pump.duty_pct);

                let sparkline = Canvas::new(Sparkline::new(self.temp_history.iter().copied()))
                    .width(Length::Fixed(36.0))
                    .height(Length::Fixed(16.0));

                let coolant_glyph = row![
                    symbolic_icon(ICON_SNOWFLAKE),
                    symbolic_icon(ICON_TEMP),
                ]
                .spacing(1)
                .align_y(Alignment::Center);

                row![
                    coolant_glyph,
                    self.core.applet.text(temp_text).font(cosmic::font::mono()),
                    sparkline,
                    symbolic_icon(ICON_FAN),
                    self.core.applet.text(fan_text).font(cosmic::font::mono()),
                    symbolic_icon(ICON_PUMP),
                    self.core.applet.text(pump_text).font(cosmic::font::mono()),
                ]
                .spacing(4)
                .align_y(Alignment::Center)
                .into()
            }
            (None, Some(_)) => self.core.applet.text("!").into(),
            (None, None) => self.core.applet.text("…").into(),
        };

        let pad = self.core.applet.suggested_padding(true).0;
        let button = widget::button::custom(content)
            .padding([0, pad])
            .on_press(Message::TogglePopup)
            .class(cosmic::theme::Button::AppletIcon);

        autosize::autosize(button, AUTOSIZE_ID.clone()).into()
    }

    /// The applet's popup window will be drawn using this view method. If there are
    /// multiple poups, you may match the id parameter to determine which popup to
    /// create a view for.
    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        let content: Element<'_, Self::Message> = match (&self.last_status, &self.last_error) {
            (Some(status), maybe_err) => {
                let mut column = widget::list_column();

                column = column.add(widget::text::heading(status.description.clone()));
                column = column.add(
                    widget::text::body(format!("Liquid: {:.1} °C", status.liquid_temp_c))
                        .font(cosmic::font::mono()),
                );
                column = column.add(
                    widget::text::body(format!(
                        "Pump   {} rpm   {}%",
                        status.pump.speed_rpm, status.pump.duty_pct
                    ))
                    .font(cosmic::font::mono()),
                );

                for fan in &status.fans {
                    column = column.add(
                        widget::text::body(format!(
                            "Fan {}  {} rpm   {}%",
                            fan.index, fan.speed_rpm, fan.duty_pct
                        ))
                        .font(cosmic::font::mono()),
                    );
                }

                if let Some(err) = maybe_err {
                    column = column.add(widget::text::caption(format!("error: {err}")));
                }

                column.into()
            }
            (None, Some(err)) => {
                let column = widget::list_column()
                    .add(widget::text::heading("liquidctl error".to_string()))
                    .add(widget::text::body(err.clone()));
                column.into()
            }
            (None, None) => {
                let column = widget::list_column().add(widget::text::body(
                    "Waiting for first reading…".to_string(),
                ));
                column.into()
            }
        };

        self.core.applet.popup_container(content).into()
    }

    /// Register subscriptions for this application.
    ///
    /// Subscriptions are long-lived async tasks running in the background which
    /// emit messages to the application through a channel. They may be conditionally
    /// activated by selectively appending to the subscription batch, and will
    /// continue to execute for the duration that they remain in the batch.
    fn subscription(&self) -> Subscription<Self::Message> {
        Subscription::batch(vec![
            // Poll liquidctl for status every 1500ms.
            Subscription::run_with("liquidctl-sub", |_| {
                cosmic::iced::stream::channel(
                    4,
                    |mut channel: mpsc::Sender<Message>| async move {
                        loop {
                            let result = crate::liquidctl::fetch_status("Hydro")
                                .await
                                .map_err(|e| format!("{e}"));
                            if channel.send(Message::StatusTick(result)).await.is_err() {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(1500)).await;
                        }
                        futures_util::future::pending().await
                    },
                )
            }),
            // Watch for application configuration changes.
            self.core()
                .watch_config::<Config>(Self::APP_ID)
                .map(|update| Message::UpdateConfig(update.config)),
        ])
    }

    /// Handles messages emitted by the application and its widgets.
    ///
    /// Tasks may be returned for asynchronous execution of code in the background
    /// on the application's async runtime. The application will not exit until all
    /// tasks are finished.
    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::UpdateConfig(config) => {
                self.config = config;
            }
            Message::StatusTick(Ok(status)) => {
                self.temp_history.push_back(status.liquid_temp_c);
                while self.temp_history.len() > MAX_SAMPLES {
                    self.temp_history.pop_front();
                }
                self.last_status = Some(status);
                self.last_error = None;
            }
            Message::StatusTick(Err(msg)) => {
                self.last_error = Some(msg);
                // Intentionally don't clear last_status — show stale data alongside the error.
            }
            Message::TogglePopup => {
                return if let Some(p) = self.popup.take() {
                    destroy_popup(p)
                } else {
                    let Some(parent) = self.core.main_window_id() else {
                        self.popup = None;
                        return Task::none();
                    };
                    let new_id = Id::unique();
                    self.popup.replace(new_id);
                    let mut popup_settings = self.core.applet.get_popup_settings(
                        parent,
                        new_id,
                        None,
                        None,
                        None,
                    );
                    popup_settings.positioner.size_limits = Limits::NONE
                        .max_width(372.0)
                        .min_width(300.0)
                        .min_height(200.0)
                        .max_height(1080.0);
                    get_popup(popup_settings)
                }
            }
            Message::PopupClosed(id) => {
                if self.popup.as_ref() == Some(&id) {
                    self.popup = None;
                }
            }
        }
        Task::none()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::liquidctl::{AioStatus, Fan, Pump};
    use cosmic::Application as _;

    fn fan(index: u8, duty_pct: u8) -> Fan {
        Fan {
            index,
            speed_rpm: 1000,
            duty_pct,
        }
    }

    fn sample_status(temp_c: f64) -> AioStatus {
        AioStatus {
            description: "Test AIO".to_string(),
            liquid_temp_c: temp_c,
            pump: Pump {
                speed_rpm: 2000,
                duty_pct: 70,
            },
            fans: vec![fan(1, 40), fan(2, 50), fan(3, 60)],
        }
    }

    #[test]
    fn fan_duty_avg_is_none_for_empty() {
        assert_eq!(fan_duty_avg(&[]), None);
    }

    #[test]
    fn fan_duty_avg_computes_integer_mean() {
        assert_eq!(fan_duty_avg(&[fan(1, 40), fan(2, 50), fan(3, 60)]), Some(50));
    }

    #[test]
    fn fan_duty_avg_truncates_toward_zero() {
        // 40 + 50 = 90 / 2 = 45 (exact). Use 41 + 50 = 91 / 2 = 45 (truncated).
        assert_eq!(fan_duty_avg(&[fan(1, 41), fan(2, 50)]), Some(45));
    }

    #[test]
    fn fan_duty_avg_at_max() {
        assert_eq!(
            fan_duty_avg(&[fan(1, 100), fan(2, 100), fan(3, 100)]),
            Some(100)
        );
    }

    #[test]
    fn status_tick_ok_appends_temp_and_clears_error() {
        let mut model = AppModel {
            last_error: Some("previous error".to_string()),
            ..AppModel::default()
        };

        let _ = model.update(Message::StatusTick(Ok(sample_status(30.5))));

        assert_eq!(model.temp_history.len(), 1);
        assert!((model.temp_history[0] - 30.5).abs() < 1e-9);
        let status = model.last_status.as_ref().expect("status set");
        assert!((status.liquid_temp_c - 30.5).abs() < 1e-9);
        assert!(model.last_error.is_none());
    }

    #[test]
    fn status_tick_err_preserves_stale_status() {
        let mut model = AppModel::default();
        let _ = model.update(Message::StatusTick(Ok(sample_status(31.0))));
        assert!(model.last_status.is_some());

        let _ = model.update(Message::StatusTick(Err("boom".to_string())));

        // Stale data preserved per src/app.rs:268-275 design comment.
        assert!(model.last_status.is_some());
        assert_eq!(model.last_error.as_deref(), Some("boom"));
        assert_eq!(model.temp_history.len(), 1);
    }

    #[test]
    fn temp_history_caps_at_max_samples() {
        let mut model = AppModel::default();
        for i in 0..(MAX_SAMPLES + 10) {
            let _ = model.update(Message::StatusTick(Ok(sample_status(20.0 + i as f64))));
        }
        assert_eq!(model.temp_history.len(), MAX_SAMPLES);
        // Oldest sample dropped: first retained value should be index 10 (=> 30.0).
        let first = *model.temp_history.front().unwrap();
        assert!(
            (first - 30.0).abs() < 1e-9,
            "first sample after cap should be 30.0, got {first}"
        );
        // Newest is the last one pushed.
        let last = *model.temp_history.back().unwrap();
        let expected_last = 20.0 + (MAX_SAMPLES + 10 - 1) as f64;
        assert!((last - expected_last).abs() < 1e-9);
    }

    #[test]
    fn popup_closed_with_matching_id_clears_popup() {
        let mut model = AppModel::default();
        let id = Id::unique();
        model.popup = Some(id);

        let _ = model.update(Message::PopupClosed(id));

        assert!(model.popup.is_none());
    }

    #[test]
    fn popup_closed_with_non_matching_id_is_noop() {
        let mut model = AppModel::default();
        let kept = Id::unique();
        let other = Id::unique();
        model.popup = Some(kept);

        let _ = model.update(Message::PopupClosed(other));

        assert_eq!(model.popup, Some(kept));
    }

    #[test]
    fn update_config_replaces_config() {
        let mut model = AppModel::default();
        let new_cfg = Config::default();
        let _ = model.update(Message::UpdateConfig(new_cfg));
        // Config is currently empty; just assert the arm runs and doesn't disturb other state.
        assert!(model.last_status.is_none());
        assert!(model.last_error.is_none());
        assert!(model.temp_history.is_empty());
    }
}
