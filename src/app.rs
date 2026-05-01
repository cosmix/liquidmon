// SPDX-License-Identifier: MPL-2.0

use crate::config::Config;
use crate::devices;
use crate::liquidctl::DetectedDevice;
use crate::sparkline::{Sparkline, SparklineTint};
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
    /// Devices observed at the last `liquidctl list` enumeration. Filtered to
    /// AIOs via `devices::filter_aios` when used by the dropdown.
    detected_devices: Vec<DetectedDevice>,
    /// True while a `liquidctl list` task is in flight, so the popup can
    /// show a "Detecting devices…" placeholder and avoid concurrent requests.
    device_scan_in_flight: bool,
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
    /// Result of a `liquidctl list --json` enumeration.
    DevicesEnumerated(Result<Vec<DetectedDevice>, String>),
    /// User chose a device from the popup dropdown. `None` means revert to Auto.
    DeviceSelected(Option<String>),
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
            device_scan_in_flight: true,
            ..Default::default()
        };

        let init_task = Task::perform(
            async {
                crate::liquidctl::list_devices()
                    .await
                    .map_err(|e| format!("{e}"))
            },
            |r| cosmic::Action::App(Message::DevicesEnumerated(r)),
        );
        (app, init_task)
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
                // Tint the panel sparkline with the panel foreground color so
                // it stays visible regardless of wallpaper. Popup sparklines
                // (built in `popup_metrics_view`) keep the accent default.
                let sparkline =
                    Canvas::new(Sparkline::new(panel_iter).with_tint(SparklineTint::OnPanel))
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
        // Subscription identity is the (data, fn-pointer) pair. Keying on
        // `(interval_ms, match_str)` means iced tears down and restarts the
        // poll loop when EITHER the user commits a new interval OR picks a
        // different device. Until enumeration resolves an effective match,
        // we install no poll subscription so no spurious "no AIO detected"
        // error is surfaced before init's enumerate task lands.
        let mut subs: Vec<Subscription<Message>> = vec![
            self.core()
                .watch_config::<Config>(Self::APP_ID)
                .map(|update| Message::UpdateConfig(update.config)),
        ];

        if let Some(match_str) = self.effective_match() {
            let interval_ms = self
                .config
                .sample_interval_ms
                .clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS);
            let key: (u64, String) = (interval_ms, match_str);

            subs.push(Subscription::run_with(key, |key: &(u64, String)| {
                let interval_ms = key.0;
                let match_str = key.1.clone();
                cosmic::iced::stream::channel(
                    4,
                    move |mut channel: mpsc::Sender<Message>| async move {
                        loop {
                            let result = crate::liquidctl::fetch_status(&match_str)
                                .await
                                .map_err(|e| format!("{e}"));
                            if channel.send(Message::StatusTick(result)).await.is_err() {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(interval_ms)).await;
                        }
                        futures_util::future::pending().await
                    },
                )
            }));
        }

        Subscription::batch(subs)
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
                    let get_popup_task = get_popup(popup_settings);
                    if self.device_scan_in_flight {
                        get_popup_task
                    } else {
                        // Hot-plug refresh on popup open. Already gated by
                        // `device_scan_in_flight` above; setting it here
                        // prevents a second concurrent enumerate if the user
                        // toggles the popup faster than `list` returns.
                        self.device_scan_in_flight = true;
                        let enumerate = Task::perform(
                            async {
                                crate::liquidctl::list_devices()
                                    .await
                                    .map_err(|e| format!("{e}"))
                            },
                            |r| cosmic::Action::App(Message::DevicesEnumerated(r)),
                        );
                        Task::batch(vec![get_popup_task, enumerate])
                    }
                };
            }
            Message::PopupClosed(id) => {
                if self.popup.as_ref() == Some(&id) {
                    self.popup = None;
                }
            }
            Message::DevicesEnumerated(Ok(devs)) => {
                let prev_effective = self.effective_match();
                self.detected_devices = devs;
                self.device_scan_in_flight = false;
                let new_effective = self.effective_match();
                if prev_effective != new_effective {
                    self.reset_device_state();
                }
                if new_effective.is_none() {
                    self.last_error = Some(
                        "no supported AIO detected — open the popup to select a device".to_string(),
                    );
                }
            }
            Message::DevicesEnumerated(Err(msg)) => {
                self.last_error = Some(msg);
                self.device_scan_in_flight = false;
            }
            Message::DeviceSelected(choice) => {
                if self.config.device_match != choice {
                    let prev_effective = self.effective_match();
                    self.config.device_match = choice;
                    let new_effective = self.effective_match();
                    if prev_effective != new_effective {
                        self.reset_device_state();
                    }
                    if let Some(handle) = self.config_handle.as_ref() {
                        let _ = self.config.write_entry(handle);
                    }
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
        let pump_text = format!(
            "{rpm:>5} rpm   {duty:>3} %",
            rpm = status.pump.speed_rpm,
            duty = status.pump.duty_pct,
        );

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
        sections.push(self.fans_section(status));
        sections.push(
            column![
                widget::text::body(format!("Sample interval: {secs:.1} s")),
                slider,
            ]
            .spacing(4)
            .into(),
        );
        sections.push(self.device_dropdown_section());
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

    /// Fans section: average-duty sparkline, average row in body mono, then
    /// one mono caption row per fan with right-aligned numeric columns so the
    /// rpm and duty values line up vertically — non-uniform fans stay easy to
    /// spot at a glance. Falls back to a single em-dash row when the device
    /// reports no fans.
    fn fans_section<'a>(&'a self, status: &'a crate::liquidctl::AioStatus) -> Element<'a, Message> {
        let sparkline = Canvas::new(Sparkline::new(self.fan_avg_duty_history.iter().copied()))
            .width(Length::Fixed(320.0))
            .height(Length::Fixed(80.0));

        // Single shared row format. With the mono font, identical column
        // widths in the format string translate directly to vertical column
        // alignment between the avg row and every per-fan row.
        let row_fmt = |label: &str, rpm: u32, duty: u8| -> String {
            format!("{label:<6}{rpm:>5} rpm   {duty:>3} %")
        };

        let avg_line = match (fan_speed_avg(&status.fans), fan_duty_avg(&status.fans)) {
            (Some(rpm), Some(pct)) => row_fmt("avg", rpm, pct),
            _ => "—".to_string(),
        };

        let mut children: Vec<Element<'a, Message>> = vec![
            widget::text::caption("Fans").into(),
            sparkline.into(),
            widget::text::caption(avg_line)
                .font(cosmic::font::mono())
                .into(),
        ];

        // Per-fan rows live in a tightly-spaced sub-column so the breakdown
        // reads as a compact group. They share font + format string with the
        // avg row above so the rpm and duty digits line up vertically.
        if !status.fans.is_empty() {
            let mut fan_rows: Vec<Element<'a, Message>> = Vec::with_capacity(status.fans.len());
            for fan in &status.fans {
                fan_rows.push(
                    widget::text::caption(row_fmt(
                        &format!("fan {}", fan.index),
                        fan.speed_rpm,
                        fan.duty_pct,
                    ))
                    .font(cosmic::font::mono())
                    .into(),
                );
            }
            children.push(
                cosmic::iced::widget::Column::with_children(fan_rows)
                    .spacing(1)
                    .into(),
            );
        }

        cosmic::iced::widget::Column::with_children(children)
            .spacing(4)
            .into()
    }

    /// Clear all per-device state when the effective device changes so
    /// sparklines and last-status reflect only samples from the new device.
    fn reset_device_state(&mut self) {
        self.temp_history.clear();
        self.pump_duty_history.clear();
        self.fan_avg_duty_history.clear();
        self.last_status = None;
        self.last_error = None;
    }

    /// Resolve the effective `--match` filter: user's saved choice takes
    /// precedence; otherwise fall back to the auto-selected AIO from the
    /// last enumeration. `None` means no AIO is available.
    fn effective_match(&self) -> Option<String> {
        self.config
            .device_match
            .clone()
            .or_else(|| devices::auto_select(&self.detected_devices).map(|d| d.description.clone()))
    }

    /// Build the device dropdown items: `Auto (...)` at index 0, then each
    /// connected AIO that isn't the auto-pick (the auto entry already names
    /// it), then optionally a synthetic `<saved> (disconnected)` row if the
    /// saved choice isn't currently connected.
    fn device_dropdown_items(&self) -> Vec<String> {
        let aios: Vec<&DetectedDevice> = devices::filter_aios(&self.detected_devices);
        let auto_pick = devices::auto_select(&self.detected_devices);
        let auto_label = match auto_pick {
            Some(d) => format!("Auto ({})", d.description),
            None => "Auto (no AIO detected)".to_string(),
        };

        let mut items = vec![auto_label];
        let auto_desc = auto_pick.map(|d| d.description.as_str());
        items.extend(
            aios.iter()
                .filter(|d| Some(d.description.as_str()) != auto_desc)
                .map(|d| d.description.clone()),
        );

        if let Some(saved) = self.config.device_match.as_ref()
            && !aios.iter().any(|d| &d.description == saved)
        {
            items.push(format!("{saved} (disconnected)"));
        }
        items
    }

    /// Resolve the currently-selected dropdown index. `None` (saved choice
    /// is unset) maps to index 0 (Auto). A saved choice that matches the
    /// auto-pick also maps to Auto, since the explicit row is intentionally
    /// hidden from the list to avoid duplicating the device under both Auto
    /// and a standalone entry. Otherwise the saved choice maps to its row
    /// (connected or `(disconnected)` synthetic).
    fn device_dropdown_selected(&self, items: &[String]) -> Option<usize> {
        match self.config.device_match.as_deref() {
            None => Some(0),
            Some(saved) => {
                let auto_desc =
                    devices::auto_select(&self.detected_devices).map(|d| d.description.as_str());
                if auto_desc == Some(saved) {
                    return Some(0);
                }
                items
                    .iter()
                    .position(|s| s == saved || s == &format!("{saved} (disconnected)"))
            }
        }
    }

    /// Build the device-selector section of the popup: a `Device` label
    /// over a dropdown, or a status caption while detection is pending or
    /// no AIO is connected.
    fn device_dropdown_section<'a>(&'a self) -> Element<'a, Message> {
        if self.detected_devices.is_empty() && self.config.device_match.is_none() {
            let caption = if self.device_scan_in_flight {
                "Detecting devices…"
            } else {
                "No supported AIO detected"
            };
            return column![widget::text::body("Device"), widget::text::caption(caption)]
                .spacing(4)
                .into();
        }

        let items = self.device_dropdown_items();
        let selected = self.device_dropdown_selected(&items);
        // The dropdown's `on_selected` closure must be 'static + Send + Sync,
        // so it cannot borrow `items`. Clone the list into the closure and
        // index it there to recover the chosen description.
        let items_for_closure = items.clone();
        let dropdown = cosmic::widget::dropdown(items, selected, move |idx: usize| -> Message {
            let choice = if idx == 0 {
                None
            } else {
                items_for_closure
                    .get(idx)
                    .map(|s| s.strip_suffix(" (disconnected)").unwrap_or(s).to_string())
            };
            Message::DeviceSelected(choice)
        });

        column![widget::text::body("Device"), dropdown]
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
            device_match: None,
        };
        let _ = model.update(Message::UpdateConfig(new_cfg));
        assert_eq!(model.config.sample_interval_ms, 5000);
        assert_eq!(model.config.device_match, None);
        assert!(model.last_status.is_none());
        assert!(model.last_error.is_none());
        assert!(model.temp_history.is_empty());
    }

    fn detected(description: &str) -> DetectedDevice {
        DetectedDevice {
            description: description.to_string(),
            bus: "hid".to_string(),
            address: "/dev/hidraw0".to_string(),
        }
    }

    #[test]
    fn update_config_preserves_device_match_when_replaced() {
        let mut model = AppModel::default();
        let new_cfg = Config {
            sample_interval_ms: 2000,
            device_match: Some("Corsair Hydro H150i Pro XT".to_string()),
        };
        let _ = model.update(Message::UpdateConfig(new_cfg));
        assert_eq!(
            model.config.device_match.as_deref(),
            Some("Corsair Hydro H150i Pro XT"),
        );
        assert_eq!(model.config.sample_interval_ms, 2000);
    }

    #[test]
    fn device_selected_some_persists_choice() {
        let mut model = AppModel::default();
        let _ = model.update(Message::DeviceSelected(Some(
            "Corsair Hydro H150i Pro XT".to_string(),
        )));
        assert_eq!(
            model.config.device_match.as_deref(),
            Some("Corsair Hydro H150i Pro XT"),
        );
    }

    #[test]
    fn device_selected_none_clears_choice() {
        let mut model = AppModel::default();
        model.config.device_match = Some("Corsair Hydro H150i Pro XT".to_string());
        let _ = model.update(Message::DeviceSelected(None));
        assert_eq!(model.config.device_match, None);
    }

    #[test]
    fn device_selected_same_value_is_noop() {
        let mut model = AppModel::default();
        model.config.device_match = Some("Corsair Hydro X".to_string());
        // Seed history so we can prove it survived (no reset on no-op).
        push_capped(&mut model.temp_history, 30.0);
        let _ = model.update(Message::DeviceSelected(Some("Corsair Hydro X".to_string())));
        assert_eq!(
            model.config.device_match.as_deref(),
            Some("Corsair Hydro X"),
        );
        assert_eq!(model.temp_history.len(), 1);
    }

    #[test]
    fn device_selected_change_resets_history() {
        let mut model = AppModel {
            detected_devices: vec![detected("Corsair Hydro Foo"), detected("Corsair iCUE Hbar")],
            ..AppModel::default()
        };
        let _ = model.update(Message::StatusTick(Ok(sample_status(30.0))));
        assert_eq!(model.temp_history.len(), 1);
        assert!(model.last_status.is_some());

        let _ = model.update(Message::DeviceSelected(Some(
            "Corsair iCUE Hbar".to_string(),
        )));

        assert!(model.temp_history.is_empty());
        assert!(model.pump_duty_history.is_empty());
        assert!(model.fan_avg_duty_history.is_empty());
        assert!(model.last_status.is_none());
    }

    #[test]
    fn device_selected_to_auto_when_auto_resolves_to_same_does_not_reset() {
        // Auto picks "Corsair Hydro Foo" because it's the first AIO.
        let mut model = AppModel {
            detected_devices: vec![detected("Corsair Hydro Foo")],
            ..AppModel::default()
        };
        let _ = model.update(Message::StatusTick(Ok(sample_status(30.0))));
        assert_eq!(model.temp_history.len(), 1);

        // Explicitly pick the same description that auto would resolve to —
        // the effective match is unchanged so histories must survive.
        let _ = model.update(Message::DeviceSelected(Some(
            "Corsair Hydro Foo".to_string(),
        )));

        assert_eq!(model.temp_history.len(), 1);
        assert!(model.last_status.is_some());
    }

    #[test]
    fn devices_enumerated_ok_replaces_list() {
        let mut model = AppModel {
            device_scan_in_flight: true,
            ..AppModel::default()
        };
        let _ = model.update(Message::DevicesEnumerated(Ok(vec![
            detected("Corsair Hydro Foo"),
            detected("Some RGB Hub"),
        ])));
        assert_eq!(model.detected_devices.len(), 2);
        assert!(!model.device_scan_in_flight);
    }

    #[test]
    fn devices_enumerated_change_in_auto_resets_history() {
        let mut model = AppModel::default();
        // Seed histories as if a previous device had been polling.
        push_capped(&mut model.temp_history, 30.0);
        push_capped(&mut model.pump_duty_history, 70.0);
        push_capped(&mut model.fan_avg_duty_history, 50.0);
        model.last_status = Some(sample_status(30.0));

        // Now an enumerate completes and auto-detect picks an AIO where
        // none was selected before — effective match transitions
        // None → Some, so histories must clear.
        let _ = model.update(Message::DevicesEnumerated(Ok(vec![detected(
            "Corsair Hydro Foo",
        )])));

        assert!(model.temp_history.is_empty());
        assert!(model.pump_duty_history.is_empty());
        assert!(model.fan_avg_duty_history.is_empty());
        assert!(model.last_status.is_none());
    }

    #[test]
    fn devices_enumerated_no_aio_sets_error() {
        let mut model = AppModel::default();
        let _ = model.update(Message::DevicesEnumerated(Ok(vec![])));
        assert!(model.last_error.is_some());
        assert!(
            model
                .last_error
                .as_deref()
                .unwrap()
                .contains("no supported AIO detected"),
        );
    }

    #[test]
    fn devices_enumerated_err_sets_error_preserves_status() {
        let mut model = AppModel::default();
        // Drive a successful StatusTick first to populate last_status.
        let _ = model.update(Message::StatusTick(Ok(sample_status(30.0))));
        assert!(model.last_status.is_some());

        let _ = model.update(Message::DevicesEnumerated(Err("boom".to_string())));

        assert_eq!(model.last_error.as_deref(), Some("boom"));
        assert!(model.last_status.is_some());
        assert!(!model.device_scan_in_flight);
    }

    #[test]
    fn effective_match_prefers_user_choice_over_auto() {
        let mut model = AppModel {
            detected_devices: vec![detected("Corsair Hydro Foo")],
            ..AppModel::default()
        };
        model.config.device_match = Some("Corsair iCUE Hbar".to_string());
        assert_eq!(
            model.effective_match().as_deref(),
            Some("Corsair iCUE Hbar"),
        );
    }

    #[test]
    fn effective_match_falls_back_to_auto_when_unset() {
        let model = AppModel {
            detected_devices: vec![detected("Corsair Hydro Foo")],
            ..AppModel::default()
        };
        assert_eq!(
            model.effective_match().as_deref(),
            Some("Corsair Hydro Foo"),
        );
    }

    #[test]
    fn effective_match_is_none_when_no_aio_detected() {
        let model = AppModel::default();
        assert!(model.effective_match().is_none());
    }

    #[test]
    fn effective_match_honors_saved_when_disconnected() {
        let mut model = AppModel::default();
        // No detected devices, but a saved choice. effective_match must
        // still return Some so the poll subscription keeps trying.
        model.config.device_match = Some("Corsair Hydro Lost".to_string());
        assert_eq!(
            model.effective_match().as_deref(),
            Some("Corsair Hydro Lost"),
        );
    }

    #[test]
    fn dropdown_items_includes_auto_first() {
        let model = AppModel {
            detected_devices: vec![detected("Corsair Hydro Foo")],
            ..AppModel::default()
        };
        let items = model.device_dropdown_items();
        assert!(!items.is_empty());
        assert!(
            items[0].starts_with("Auto"),
            "first item should be Auto, got {:?}",
            items[0],
        );
    }

    #[test]
    fn dropdown_items_appends_disconnected_synthetic_when_saved_missing() {
        let mut model = AppModel {
            detected_devices: vec![detected("Corsair Hydro Foo")],
            ..AppModel::default()
        };
        model.config.device_match = Some("Corsair iCUE Hbar".to_string());
        let items = model.device_dropdown_items();
        assert!(
            items
                .iter()
                .any(|s| s == "Corsair iCUE Hbar (disconnected)"),
            "expected disconnected synthetic in {items:?}",
        );
    }

    #[test]
    fn dropdown_items_omits_synthetic_when_saved_is_connected() {
        let mut model = AppModel {
            detected_devices: vec![detected("Corsair Hydro Foo")],
            ..AppModel::default()
        };
        model.config.device_match = Some("Corsair Hydro Foo".to_string());
        let items = model.device_dropdown_items();
        assert!(
            !items.iter().any(|s| s.ends_with("(disconnected)")),
            "no disconnected row when saved is connected, got {items:?}",
        );
    }

    #[test]
    fn dropdown_items_omits_auto_picked_device_from_explicit_list() {
        // Auto label already names the auto-picked device — listing it
        // again as its own row would be redundant.
        let model = AppModel {
            detected_devices: vec![detected("Corsair Hydro Foo")],
            ..AppModel::default()
        };
        let items = model.device_dropdown_items();
        assert_eq!(
            items.len(),
            1,
            "single AIO should yield one item (the Auto row only), got {items:?}",
        );
        assert!(items[0].starts_with("Auto ("));
    }

    #[test]
    fn dropdown_items_lists_non_auto_aios_explicitly() {
        // With multiple AIOs, only the auto-pick is hidden — the others
        // remain available as explicit rows.
        let model = AppModel {
            detected_devices: vec![detected("Corsair Hydro Foo"), detected("Corsair iCUE Hbar")],
            ..AppModel::default()
        };
        let items = model.device_dropdown_items();
        assert_eq!(items.len(), 2, "got {items:?}");
        assert!(items[0].starts_with("Auto (Corsair Hydro Foo)"));
        assert_eq!(items[1], "Corsair iCUE Hbar");
    }
}
