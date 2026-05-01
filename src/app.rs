// SPDX-License-Identifier: MPL-2.0

use crate::config::Config;
use crate::sparkline::Sparkline;
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::futures::channel::mpsc;
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::widget::canvas::Canvas;
use cosmic::iced::widget::{column, row};
use cosmic::iced::{Alignment, Length, Limits, Subscription, window::Id};
use cosmic::prelude::*;
use cosmic::widget;
use cosmic::widget::autosize;
use futures_util::SinkExt;
use std::collections::VecDeque;
use std::sync::LazyLock;
use std::time::Duration;

static AUTOSIZE_ID: LazyLock<widget::Id> = LazyLock::new(|| widget::Id::new("liquidmon-applet"));

const PANEL_SPARK_SAMPLES: usize = 60;
const HISTORY_CAP: usize = 900;
const MIN_INTERVAL_MS: u64 = 1000;
const MAX_INTERVAL_MS: u64 = 10000;

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

fn fan_speed_avg(fans: &[crate::liquidctl::Fan]) -> Option<u32> {
    if fans.is_empty() {
        return None;
    }
    let sum: u64 = fans.iter().map(|f| u64::from(f.speed_rpm)).sum();
    Some((sum / fans.len() as u64) as u32)
}

/// Push a new sample onto a metric history, evicting from the front to keep
/// the buffer bounded at `HISTORY_CAP`.
fn push_capped(buf: &mut VecDeque<f64>, value: f64) {
    buf.push_back(value);
    while buf.len() > HISTORY_CAP {
        buf.pop_front();
    }
}

/// Build one popup metric section: caption label, sparkline canvas, numeric
/// body. Extracted so `view_window` stays under the file's 50-line per-fn
/// soft limit.
fn metric_section<'a>(
    label: &'a str,
    history: &VecDeque<f64>,
    height: f32,
    value_text: String,
) -> Element<'a, Message> {
    let sparkline = Canvas::new(Sparkline::new(history.iter().copied()))
        .width(Length::Fixed(320.0))
        .height(Length::Fixed(height));

    column![
        widget::text::caption(label),
        sparkline,
        widget::text::body(value_text).font(cosmic::font::mono()),
    ]
    .spacing(4)
    .into()
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
    /// Live cosmic-config handle held so `write_entry` can persist updates
    /// initiated from the popup slider. `None` if the config service was
    /// unavailable at startup.
    config_handle: Option<cosmic_config::Config>,
    /// Slider value while the user is mid-drag, in seconds. `None` outside a
    /// drag — release commits this into `config.sample_interval_ms` so the
    /// subscription key stays stable during the drag.
    pending_interval_secs: Option<f32>,
    /// The most recent successful liquidctl status reading.
    last_status: Option<crate::liquidctl::AioStatus>,
    /// The most recent error message, if any.
    last_error: Option<String>,
    /// Liquid temperature samples (oldest first).
    temp_history: VecDeque<f64>,
    /// Pump duty samples in percent (oldest first).
    pump_duty_history: VecDeque<f64>,
    /// Mean fan duty across all fans, in percent (oldest first). Skipped on
    /// ticks with no fans so the y-axis auto-scaler isn't dragged toward zero.
    fan_avg_duty_history: VecDeque<f64>,
}

/// Messages emitted by the application and its widgets.
#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    PopupClosed(Id),
    UpdateConfig(Config),
    StatusTick(Result<crate::liquidctl::AioStatus, String>),
    /// Fired continuously while the user drags the sample-interval slider.
    SampleIntervalDragged(f32),
    /// Fired once when the slider is released — commits and persists.
    SampleIntervalReleased,
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
        // Build the handle once and reuse for both reading the entry and
        // (later) persisting writes from the slider.
        let config_handle = cosmic_config::Config::new(Self::APP_ID, Config::VERSION).ok();
        let config = config_handle
            .as_ref()
            .map(|ctx| match Config::get_entry(ctx) {
                Ok(c) => c,
                Err((_errors, c)) => c,
            })
            .unwrap_or_default();

        let app = AppModel {
            core,
            config,
            config_handle,
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

                // Feed only the most recent PANEL_SPARK_SAMPLES so the panel
                // glyph stays a short-window trend even though we keep a much
                // longer history for the popup.
                let panel_iter = self
                    .temp_history
                    .iter()
                    .copied()
                    .skip(self.temp_history.len().saturating_sub(PANEL_SPARK_SAMPLES));
                let sparkline = Canvas::new(Sparkline::new(panel_iter))
                    .width(Length::Fixed(36.0))
                    .height(Length::Fixed(16.0));

                let coolant_glyph = row![symbolic_icon(ICON_SNOWFLAKE), symbolic_icon(ICON_TEMP),]
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
            (Some(status), maybe_err) => self.popup_metrics_view(status, maybe_err.as_deref()),
            (None, Some(err)) => widget::list_column()
                .add(widget::text::heading("liquidctl error".to_string()))
                .add(widget::text::body(err.clone()))
                .into(),
            (None, None) => widget::list_column()
                .add(widget::text::body("Waiting for first reading…".to_string()))
                .into(),
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
        // Subscription identity is the (data, fn-pointer) pair: keying on the
        // interval means iced tears down and restarts the poll loop only when
        // the user commits a new interval (drag-release), not on every drag
        // tick. `run_with` requires a real fn pointer, so we cannot capture —
        // the interval rides through as `data`.
        let interval_ms = self
            .config
            .sample_interval_ms
            .clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS);

        let liquidctl_sub = Subscription::run_with(interval_ms, |interval_ms: &u64| {
            let interval_ms = *interval_ms;
            cosmic::iced::stream::channel(4, move |mut channel: mpsc::Sender<Message>| async move {
                loop {
                    let result = crate::liquidctl::fetch_status("Hydro")
                        .await
                        .map_err(|e| format!("{e}"));
                    if channel.send(Message::StatusTick(result)).await.is_err() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(interval_ms)).await;
                }
                futures_util::future::pending().await
            })
        });

        Subscription::batch(vec![
            liquidctl_sub,
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
                push_capped(&mut self.temp_history, status.liquid_temp_c);
                push_capped(&mut self.pump_duty_history, f64::from(status.pump.duty_pct));
                // Skip the push entirely when no fans are reported — pushing
                // 0.0 would corrupt the auto-scaled y-axis on the next tick.
                if let Some(pct) = fan_duty_avg(&status.fans) {
                    push_capped(&mut self.fan_avg_duty_history, f64::from(pct));
                }

                self.last_status = Some(status);
                self.last_error = None;
            }
            Message::StatusTick(Err(msg)) => {
                self.last_error = Some(msg);
                // Intentionally don't clear last_status — show stale data alongside the error.
            }
            Message::SampleIntervalDragged(secs) => {
                self.pending_interval_secs = Some(secs);
            }
            Message::SampleIntervalReleased => {
                self.commit_pending_interval();
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
                    let mut popup_settings = self
                        .core
                        .applet
                        .get_popup_settings(parent, new_id, None, None, None);
                    popup_settings.positioner.size_limits = Limits::NONE
                        .max_width(380.0)
                        .min_width(320.0)
                        .min_height(360.0)
                        .max_height(1080.0);
                    get_popup(popup_settings)
                };
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

impl AppModel {
    /// Render the populated popup body — three metric sections (coolant temp,
    /// pump duty, fan-average duty), the sample-rate slider, and an optional
    /// error caption — wrapped in a `scrollable` so constrained panels degrade
    /// to scroll instead of clipping.
    fn popup_metrics_view<'a>(
        &'a self,
        status: &'a crate::liquidctl::AioStatus,
        maybe_err: Option<&'a str>,
    ) -> Element<'a, Message> {
        let pump_text = format!("{} rpm   {} %", status.pump.speed_rpm, status.pump.duty_pct);
        let fan_text = match (fan_speed_avg(&status.fans), fan_duty_avg(&status.fans)) {
            (Some(rpm), Some(pct)) => format!("{rpm} rpm   {pct} %"),
            _ => "—".to_string(),
        };

        // Live drag value falls back to the persisted setting when not dragging.
        // The slider itself is f32; the persisted value is u64 ms — the cast is
        // narrowing only above ~16 million ms which we clamp far below.
        #[allow(clippy::cast_precision_loss)]
        let secs = self
            .pending_interval_secs
            .unwrap_or((self.config.sample_interval_ms as f32) / 1000.0);

        let slider = widget::slider(1.0..=10.0_f32, secs, Message::SampleIntervalDragged)
            .step(0.5_f32)
            .on_release(Message::SampleIntervalReleased)
            .width(Length::Fill);

        let mut sections: Vec<Element<'a, Message>> = Vec::with_capacity(6);
        sections.push(widget::text::heading(status.description.clone()).into());
        sections.push(metric_section(
            "Coolant temperature",
            &self.temp_history,
            80.0,
            format!("{:.1} °C", status.liquid_temp_c),
        ));
        sections.push(metric_section(
            "Pump",
            &self.pump_duty_history,
            80.0,
            pump_text,
        ));
        sections.push(self.fans_section(status, fan_text));
        sections.push(
            column![
                widget::text::body(format!("Sample interval: {secs:.1} s")),
                slider,
            ]
            .spacing(4)
            .into(),
        );
        if let Some(err) = maybe_err {
            sections.push(widget::text::caption(format!("error: {err}")).into());
        }

        widget::scrollable(
            cosmic::iced::widget::Column::with_children(sections)
                .spacing(12)
                .padding(12),
        )
        .into()
    }

    /// Fans section: average duty sparkline + average rpm/duty body, plus a
    /// compact per-fan rpm/duty line so non-uniform fans stay visible at a
    /// glance. Falls back to the bare avg row when the device reports no fans.
    fn fans_section<'a>(
        &'a self,
        status: &'a crate::liquidctl::AioStatus,
        avg_text: String,
    ) -> Element<'a, Message> {
        let sparkline = Canvas::new(Sparkline::new(self.fan_avg_duty_history.iter().copied()))
            .width(Length::Fixed(320.0))
            .height(Length::Fixed(80.0));

        let mut children: Vec<Element<'a, Message>> = vec![
            widget::text::caption("Fans (avg)").into(),
            sparkline.into(),
            widget::text::body(avg_text)
                .font(cosmic::font::mono())
                .into(),
        ];

        if !status.fans.is_empty() {
            let per_fan = status
                .fans
                .iter()
                .map(|f| format!("{} {}rpm/{}%", f.index, f.speed_rpm, f.duty_pct))
                .collect::<Vec<_>>()
                .join("  ·  ");
            children.push(
                widget::text::caption(per_fan)
                    .font(cosmic::font::mono())
                    .into(),
            );
        }

        cosmic::iced::widget::Column::with_children(children)
            .spacing(4)
            .into()
    }

    /// Apply the staged slider value, clamp to the supported range, and persist
    /// to cosmic-config (best effort). No-op when no drag is in progress.
    fn commit_pending_interval(&mut self) {
        let Some(secs) = self.pending_interval_secs.take() else {
            return;
        };
        // Clamp the f64 ms value first — the slider range is [1.0, 10.0] so
        // (secs * 1000.0) sits in [1000, 10000] far below u64::MAX, and the
        // clamp below pulls anything else into bounds before we cast.
        #[allow(clippy::cast_precision_loss)]
        let (lo, hi) = (MIN_INTERVAL_MS as f64, MAX_INTERVAL_MS as f64);
        let ms_f = (f64::from(secs) * 1000.0).round().clamp(lo, hi);
        // Cast is safe: ms_f ∈ [1000, 10000] ⊂ u64.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ms = ms_f as u64;
        if ms != self.config.sample_interval_ms {
            self.config.sample_interval_ms = ms;
            if let Some(handle) = self.config_handle.as_ref() {
                let _ = self.config.write_entry(handle);
            }
        }
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

    fn fan_with_speed(index: u8, speed_rpm: u32) -> Fan {
        Fan {
            index,
            speed_rpm,
            duty_pct: 50,
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
        assert_eq!(
            fan_duty_avg(&[fan(1, 40), fan(2, 50), fan(3, 60)]),
            Some(50)
        );
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
    fn fan_speed_avg_computes_integer_mean() {
        assert_eq!(
            fan_speed_avg(&[
                fan_with_speed(1, 1000),
                fan_with_speed(2, 2000),
                fan_with_speed(3, 3000),
            ]),
            Some(2000)
        );
        assert_eq!(fan_speed_avg(&[]), None);
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

        // Stale data preserved: error is shown alongside the last good reading.
        assert!(model.last_status.is_some());
        assert_eq!(model.last_error.as_deref(), Some("boom"));
        assert_eq!(model.temp_history.len(), 1);
    }

    #[test]
    fn temp_history_caps_at_history_cap() {
        let mut model = AppModel::default();
        for i in 0..(HISTORY_CAP + 10) {
            let _ = model.update(Message::StatusTick(Ok(sample_status(20.0 + i as f64))));
        }
        assert_eq!(model.temp_history.len(), HISTORY_CAP);
        // Oldest sample dropped: first retained value should be index 10 (=> 30.0).
        let first = *model.temp_history.front().unwrap();
        assert!(
            (first - 30.0).abs() < 1e-9,
            "first sample after cap should be 30.0, got {first}"
        );
        // Newest is the last one pushed.
        let last = *model.temp_history.back().unwrap();
        let expected_last = 20.0 + (HISTORY_CAP + 10 - 1) as f64;
        assert!((last - expected_last).abs() < 1e-9);
    }

    #[test]
    fn status_tick_ok_appends_to_all_metric_histories() {
        let mut model = AppModel::default();
        let _ = model.update(Message::StatusTick(Ok(sample_status(30.0))));

        assert_eq!(model.temp_history.len(), 1);
        assert_eq!(model.pump_duty_history.len(), 1);
        // sample_status has 3 fans → fan-avg-duty history should grow.
        assert_eq!(model.fan_avg_duty_history.len(), 1);

        // Fan averages: duty 40+50+60 → 50.
        assert!((model.fan_avg_duty_history[0] - 50.0).abs() < 1e-9);
    }

    #[test]
    fn status_tick_with_no_fans_skips_fan_history_push() {
        let mut model = AppModel::default();
        let mut status = sample_status(28.0);
        status.fans.clear();
        let _ = model.update(Message::StatusTick(Ok(status)));

        assert_eq!(model.temp_history.len(), 1);
        assert_eq!(model.pump_duty_history.len(), 1);
        assert!(model.fan_avg_duty_history.is_empty());
    }

    #[test]
    fn sample_interval_dragged_stages_pending_value() {
        let mut model = AppModel::default();
        let _ = model.update(Message::SampleIntervalDragged(2.5));
        assert_eq!(model.pending_interval_secs, Some(2.5));
        // Dragging alone must NOT mutate persisted config — keeps the
        // subscription identity stable during the drag.
        assert_eq!(model.config.sample_interval_ms, 1500);
    }

    #[test]
    fn sample_interval_released_commits_clamped_value() {
        let mut model = AppModel::default();
        let _ = model.update(Message::SampleIntervalDragged(3.0));
        let _ = model.update(Message::SampleIntervalReleased);
        assert_eq!(model.config.sample_interval_ms, 3000);
        assert_eq!(model.pending_interval_secs, None);
    }

    #[test]
    fn sample_interval_released_clamps_above_max() {
        let mut model = AppModel::default();
        let _ = model.update(Message::SampleIntervalDragged(99.0));
        let _ = model.update(Message::SampleIntervalReleased);
        assert_eq!(model.config.sample_interval_ms, MAX_INTERVAL_MS);
    }

    #[test]
    fn sample_interval_released_clamps_below_min() {
        let mut model = AppModel::default();
        let _ = model.update(Message::SampleIntervalDragged(0.1));
        let _ = model.update(Message::SampleIntervalReleased);
        assert_eq!(model.config.sample_interval_ms, MIN_INTERVAL_MS);
    }

    #[test]
    fn sample_interval_released_without_drag_is_noop() {
        let mut model = AppModel::default();
        let original = model.config.sample_interval_ms;
        let _ = model.update(Message::SampleIntervalReleased);
        assert_eq!(model.config.sample_interval_ms, original);
        assert_eq!(model.pending_interval_secs, None);
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
        let new_cfg = Config {
            sample_interval_ms: 5000,
        };
        let _ = model.update(Message::UpdateConfig(new_cfg));
        assert_eq!(model.config.sample_interval_ms, 5000);
        assert!(model.last_status.is_none());
        assert!(model.last_error.is_none());
        assert!(model.temp_history.is_empty());
    }
}
