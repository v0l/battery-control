#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

mod demo;
mod worker;

use battery_control::{
    AuthInput, AuthState, BatteryStatus, Capabilities, Command as Control, DeviceClass,
    DeviceInfo, Reading, Setting, SettingKind, SettingValue, Unit,
};
use egui::{Color32, RichText};
use egui_bench::prelude::{
    Channel, Comb, Key, Line, card, chip, choice, field, hero, legend, lamp, note, readouts,
    reading, segments, theme, toggle,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};
use worker::{Command, Row, Update, Worker};

const HISTORY: usize = 1800;
const NOTICE_FOR: Duration = Duration::from_secs(10);
const PENDING_FOR: Duration = Duration::from_secs(8);
const SIDE_W: f32 = 290.0;
const PLOT_H: f32 = 110.0;
const LABEL_W: f32 = 190.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Plot {
    Voltage,
    Current,
    Charge,
    Power,
}

#[derive(Clone, Copy)]
struct Sample {
    volts: Option<f32>,
    amps: Option<f32>,
    soc: Option<f32>,
    watts: Option<f32>,
}

impl Sample {
    fn of(status: &BatteryStatus) -> Self {
        let volts = status.voltage().map(|v| v as f32);
        let amps = status.current().map(|v| v as f32);
        let watts = match (status.power_in(), status.power_out(), volts, amps) {
            (Some(i), Some(o), ..) => Some((i - o) as f32),
            (Some(i), None, ..) => Some(i as f32),
            (None, Some(o), ..) => Some(-(o as f32)),
            (None, None, Some(v), Some(a)) => Some(v * a),
            _ => None,
        };
        Self { volts, amps, soc: status.soc().map(|v| v as f32), watts }
    }

    fn of_plot(&self, plot: Plot) -> Option<f32> {
        match plot {
            Plot::Voltage => self.volts,
            Plot::Current => self.amps,
            Plot::Charge => self.soc,
            Plot::Power => self.watts,
        }
    }
}

impl Plot {
    fn unit(self) -> &'static str {
        match self {
            Plot::Voltage => "V",
            Plot::Current => "A",
            Plot::Charge => "%",
            Plot::Power => "W",
        }
    }
}

enum Link {
    Idle,
    Connecting(String),
    Live,
}

struct App {
    worker: Worker,
    rows: Vec<Row>,
    selected: Option<String>,
    demo: bool,
    scanning: bool,
    probe_serial: bool,
    link: Link,
    info: Option<DeviceInfo>,
    caps: Capabilities,
    status: Option<BatteryStatus>,
    history: VecDeque<Sample>,
    plot: Plot,
    notice: Option<(String, Instant)>,
    pending: HashMap<String, (bool, Instant)>,
    edits: HashMap<String, String>,
    submitted: HashSet<String>,
    pairing: Option<AuthState>,
    pin: String,
}

impl App {
    fn new(ctx: egui::Context, demo: bool) -> Result<Self, String> {
        let worker = Worker::spawn(ctx, demo)?;
        worker.send(Command::Scan { serial: false });
        Ok(Self {
            worker,
            rows: Vec::new(),
            selected: demo.then(|| worker::DEMO_ID.to_string()),
            demo,
            scanning: true,
            probe_serial: false,
            link: Link::Idle,
            info: None,
            caps: Capabilities::empty(),
            status: None,
            history: VecDeque::new(),
            plot: Plot::Voltage,
            notice: None,
            pending: HashMap::new(),
            edits: HashMap::new(),
            submitted: HashSet::new(),
            pairing: None,
            pin: String::new(),
        })
    }

    fn drain(&mut self) {
        while let Ok(update) = self.worker.rx.try_recv() {
            match update {
                Update::Scanning => self.scanning = true,
                Update::Found(rows) => {
                    self.scanning = false;
                    if self.selected.as_ref().is_none_or(|s| rows.iter().all(|r| &r.id != s)) {
                        self.selected = rows.first().map(|r| r.id.clone());
                    }
                    self.rows = rows;
                    if self.demo
                        && matches!(self.link, Link::Idle)
                        && let Some(id) = self.selected.clone()
                    {
                        self.worker.send(Command::Connect(id));
                    }
                }
                Update::Connecting(label) => self.link = Link::Connecting(label),
                Update::Connected { info, caps } => {
                    self.link = Link::Live;
                    self.info = Some(info);
                    self.caps = caps;
                    self.status = None;
                    self.history.clear();
                    self.edits.clear();
                    self.submitted.clear();
                    self.pairing = None;
                    self.pin.clear();
                }
                Update::Status(status) => {
                    if self.history.len() == HISTORY {
                        self.history.pop_front();
                    }
                    self.history.push_back(Sample::of(&status));
                    for id in self.submitted.drain() {
                        self.edits.remove(&id);
                    }
                    self.pending.retain(|id, (want, at)| {
                        at.elapsed() < PENDING_FOR && live_bool(&status, id) != Some(*want)
                    });
                    self.status = Some(status);
                }
                Update::Pairing(state) => self.pairing = Some(state),
                Update::Notice(text) => {
                    self.pending.clear();
                    self.notice = Some((text, Instant::now()));
                }
                Update::Disconnected => {
                    self.link = Link::Idle;
                    self.status = None;
                    self.info = None;
                    self.caps = Capabilities::empty();
                    self.pairing = None;
                    self.pending.clear();
                }
            }
        }
        if self.notice.as_ref().is_some_and(|(_, at)| at.elapsed() > NOTICE_FOR) {
            self.notice = None;
        }
    }

    fn flip(&mut self, id: &str, on: bool) {
        self.pending.insert(id.to_string(), (on, Instant::now()));
        self.control(Control::Toggle { id: id.to_string(), on });
    }

    fn shown(&self, id: &str, live: bool) -> bool {
        self.pending.get(id).map_or(live, |(want, _)| *want)
    }

    fn control(&self, control: Control) {
        self.worker.send(Command::Control(control));
    }

    fn rescan(&mut self) {
        self.worker.send(Command::Scan { serial: self.probe_serial });
    }

    fn header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("BATTERY CONTROL")
                    .font(theme::legend_font(15.0))
                    .extra_letter_spacing(3.0)
                    .color(theme::VALUE),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some((text, _)) = &self.notice {
                    ui.label(RichText::new(text).size(11.5).color(theme::FAULT));
                } else if let Link::Connecting(label) = &self.link {
                    ui.label(legend(format!("connecting to {label}")));
                } else if self.scanning {
                    ui.label(legend("scanning"));
                    ui.add(egui::Spinner::new().size(12.0));
                }
            });
        });
    }

    fn devices_card(&mut self, ui: &mut egui::Ui) {
        let live = matches!(self.link, Link::Live);
        let connecting = matches!(self.link, Link::Connecting(_));
        let mut scan = false;
        let mut connect = None;
        let mut disconnect = false;
        let mut pick = None;
        let rows = self.rows.clone();
        let selected = self.selected.clone();
        let mut probe_serial = self.probe_serial;
        card(
            ui,
            None,
            |ui| {
                ui.label(legend("devices"));
            },
            |ui| {
                if rows.is_empty() {
                    let text = if self.scanning { "looking for batteries" } else { "nothing found" };
                    ui.label(legend(text));
                    ui.add_space(4.0);
                }
                for row in &rows {
                    let on = selected.as_deref() == Some(row.id.as_str());
                    let class = match row.class {
                        DeviceClass::Bms => "bms",
                        DeviceClass::PowerStation => "power station",
                        DeviceClass::Monitor => "monitor",
                    };
                    let r = device_row(ui, row, class, on);
                    if r.clicked() {
                        pick = Some(row.id.clone());
                    }
                    if r.double_clicked() && !live && !connecting {
                        connect = Some(row.id.clone());
                    }
                }
                ui.add_space(4.0);
                ui.add_enabled_ui(!live && !connecting, |ui| {
                    ui.checkbox(&mut probe_serial, RichText::new("probe serial ports").size(11.5))
                        .on_hover_text("Opens each USB serial port and asks it what it is");
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let can_scan = !live && !connecting && !self.scanning;
                    if ui.add(Key::new("scan").enabled(can_scan)).clicked() {
                        scan = true;
                    }
                    if live || connecting {
                        if ui.add(Key::new("disconnect").outline(theme::FAULT)).clicked() {
                            disconnect = true;
                        }
                    } else {
                        let ready = selected.is_some() && !self.scanning;
                        if ui
                            .add(Key::new("connect").solid(theme::READOUT).enabled(ready))
                            .clicked()
                        {
                            connect = selected.clone();
                        }
                    }
                });
            },
        );
        self.probe_serial = probe_serial;
        if let Some(id) = pick {
            self.selected = Some(id);
        }
        if scan {
            self.rescan();
        }
        if disconnect {
            self.worker.send(Command::Disconnect);
        }
        if let Some(id) = connect {
            self.selected = Some(id.clone());
            self.worker.send(Command::Connect(id));
        }
    }

    fn identity_card(&self, ui: &mut egui::Ui) {
        let Some(info) = &self.info else { return };
        card(
            ui,
            None,
            |ui| {
                ui.label(legend("device"));
            },
            |ui| {
                let rows = [
                    ("backend", Some(info.backend.clone())),
                    ("maker", info.manufacturer.clone()),
                    ("model", info.model.clone()),
                    ("serial", info.serial.clone()),
                    ("firmware", info.firmware.clone()),
                    ("hardware", info.hardware.clone()),
                ];
                for (label, value) in rows {
                    if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
                        reading(ui, label, value);
                    }
                }
            },
        );
    }

    fn pairing_card(&mut self, ui: &mut egui::Ui) {
        if !self.caps.contains(Capabilities::REQUIRES_AUTH) {
            return;
        }
        let state = self.pairing.clone();
        let mut pair = None;
        let mut forget = false;
        let mut pin = self.pin.clone();
        card(
            ui,
            None,
            |ui| {
                ui.label(legend("pairing"));
            },
            |ui| {
                match &state {
                    None => note(
                        ui,
                        "Pairing with the device. It will not report until it is paired.",
                        theme::LEGEND,
                    ),
                    Some(AuthState::Authed) => note(ui, "Paired.", theme::OK),
                    Some(AuthState::PendingApproval { message }) => {
                        note(ui, message.clone(), theme::READOUT)
                    }
                    Some(AuthState::PinCode { message }) => {
                        note(ui, message.clone(), theme::READOUT);
                        field(ui, &mut pin, "code");
                    }
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let label = match &state {
                        None => "pair again",
                        Some(AuthState::Authed) => "check",
                        Some(AuthState::PendingApproval { .. }) => "try again",
                        Some(AuthState::PinCode { .. }) => "send code",
                    };
                    if ui.add(Key::new(label).solid(theme::READOUT)).clicked() {
                        pair = Some(match &state {
                            Some(AuthState::PinCode { .. }) => AuthInput::Pin(pin.clone()),
                            _ => AuthInput::None,
                        });
                    }
                    if ui.add(Key::new("forget")).clicked() {
                        forget = true;
                    }
                });
            },
        );
        self.pin = pin;
        if let Some(input) = pair {
            self.worker.send(Command::Pair(input));
        }
        if forget {
            self.pairing = None;
            self.worker.send(Command::ForgetPairing);
        }
    }

    fn summary_card(&self, ui: &mut egui::Ui, status: &BatteryStatus) {
        let title = self
            .info
            .as_ref()
            .and_then(|i| i.model.clone())
            .unwrap_or_else(|| "battery".into());
        let amps = status.current();
        let state = match amps {
            Some(a) if a > 0.05 => "charging",
            Some(a) if a < -0.05 => "discharging",
            Some(_) => "idle",
            None => "online",
        };
        let faulted = !status.alarms.is_empty();
        let rail = if faulted { theme::FAULT } else { theme::TRACE };
        card(
            ui,
            Some(rail),
            |ui| {
                ui.label(RichText::new(title).size(12.5).strong().color(theme::VALUE));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if faulted {
                        lamp(ui, &format!("{} alarm", status.alarms.len()), true, true);
                    }
                    lamp(ui, state, state == "charging" || state == "discharging", false);
                });
            },
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 30.0;
                    if let Some(soc) = status.soc() {
                        hero(ui, "charge", &format!("{soc:.0}"), "%", theme::TRACE);
                    }
                    if let Some(v) = status.voltage() {
                        hero(ui, "pack", &format!("{v:.2}"), "V", theme::TRACE);
                    }
                    if let Some(a) = amps {
                        hero(ui, "current", &format!("{a:+.2}"), "A", theme::TRACE);
                    }
                });
                let mut items: Vec<(&str, String, Color32)> = Vec::new();
                let mut add = |reading: Reading, label: &'static str, tint: Color32| {
                    if let Some(v) = status.get(reading) {
                        items.push((label, format_reading(reading.unit(), v), tint));
                    }
                };
                add(Reading::PowerIn, "power in", theme::TRACE);
                add(Reading::PowerOut, "power out", theme::TRACE);
                add(Reading::Soh, "health", theme::TRACE);
                add(Reading::Cycles, "cycles", theme::TRACE);
                add(Reading::CapacityRemainingAh, "remaining", theme::TRACE);
                add(Reading::CapacityFullAh, "full", theme::TRACE);
                add(Reading::TimeRemainingH, "time left", theme::TRACE);
                add(Reading::SocLimitMax, "charge limit", theme::READOUT);
                add(Reading::SocLimitMin, "floor", theme::READOUT);
                add(Reading::ChargeCurrentLimitA, "charge max", theme::READOUT);
                add(Reading::DischargeCurrentLimitA, "discharge max", theme::READOUT);
                if status.power_in().is_none()
                    && status.power_out().is_none()
                    && let (Some(v), Some(a)) = (status.voltage(), amps)
                {
                    items.insert(0, ("power", format!("{:+.1} W", v * a), theme::TRACE));
                }
                if !items.is_empty() {
                    ui.add_space(4.0);
                    readouts(ui, &items);
                }
                if faulted {
                    ui.add_space(2.0);
                    ui.horizontal_wrapped(|ui| {
                        for alarm in &status.alarms {
                            chip(ui, alarm, theme::FAULT, true);
                        }
                    });
                }
            },
        );
    }

    fn cells_card(&self, ui: &mut egui::Ui, status: &BatteryStatus) {
        if status.cells.is_empty() {
            return;
        }
        let channels: Vec<Channel> = status
            .cells
            .iter()
            .map(|c| Channel::marked(c.voltage.unwrap_or(0.0) * 1000.0, c.balancing == Some(true)))
            .collect();
        let spread = status.cell_delta().map(|d| d * 1000.0);
        let balancing = status.cells.iter().filter(|c| c.balancing == Some(true)).count();
        card(
            ui,
            None,
            |ui| {
                ui.label(legend("cells"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(spread) = spread {
                        let tint = if spread > 100.0 { theme::READOUT } else { theme::VALUE };
                        ui.label(
                            RichText::new(format!("{spread:.0} mV"))
                                .font(theme::figure(theme::VALUE_SIZE))
                                .color(tint),
                        );
                        ui.label(legend("spread"));
                    }
                    if balancing > 0 {
                        ui.add_space(10.0);
                        ui.label(legend(format!("{balancing} balancing")));
                    }
                });
            },
            |ui| {
                ui.add(
                    Comb::new(&channels)
                        .prefix("c")
                        .format(|mv| format!("{mv:.0}"))
                        .empty("no cells reported")
                        .height(170.0),
                );
            },
        );
    }

    fn history_card(&mut self, ui: &mut egui::Ui) {
        if self.history.len() < 2 {
            return;
        }
        let mut plot = self.plot;
        let available: Vec<(Plot, &str, Option<usize>)> = [
            (Plot::Voltage, "voltage"),
            (Plot::Current, "current"),
            (Plot::Charge, "charge"),
            (Plot::Power, "power"),
        ]
        .into_iter()
        .filter(|(p, _)| self.history.iter().any(|s| s.of_plot(*p).is_some()))
        .map(|(p, l)| (p, l, None))
        .collect();
        if available.is_empty() {
            return;
        }
        if !available.iter().any(|(p, ..)| *p == plot) {
            plot = available[0].0;
        }
        let points: Vec<f32> =
            self.history.iter().filter_map(|s| s.of_plot(plot)).collect();
        let (lo, hi) = points
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        let pad = ((hi - lo) * 0.15).max(hi.abs().max(1.0) * 0.005);
        let now = points.last().copied().unwrap_or(0.0);
        let span = self.history.len() as f32 * 2.0;
        let unit = plot.unit();
        let mut chosen = plot;
        card(
            ui,
            None,
            |ui| {
                ui.label(legend("history"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(legend(format!("last {}", minutes(span))));
                });
            },
            |ui| {
                segments(ui, &mut chosen, &available);
                ui.add_space(6.0);
                ui.add(
                    egui_bench::prelude::Trace::new(&points)
                        .size(egui::vec2(ui.available_width(), PLOT_H))
                        .range(lo - pad, hi + pad)
                        .filled(true),
                );
                Line::new()
                    .legend("now")
                    .measured(format!("{now:.2} {unit}"))
                    .gap(18.0)
                    .legend("low")
                    .value(format!("{lo:.2}"))
                    .gap(18.0)
                    .legend("high")
                    .value(format!("{hi:.2}"))
                    .show(ui);
            },
        );
        self.plot = chosen;
    }

    fn temperature_card(&self, ui: &mut egui::Ui, status: &BatteryStatus) {
        let probes: Vec<(String, String, Color32)> = status
            .sensors
            .iter()
            .filter(|s| s.unit == Unit::Celsius)
            .map(|s| (nice(s.label.as_deref().unwrap_or(&s.id)), format!("{:.1} °C", s.value), theme::TRACE))
            .collect();
        let others: Vec<(String, String)> = status
            .sensors
            .iter()
            .filter(|s| s.unit != Unit::Celsius && s.id.parse::<Reading>().is_err())
            .map(|s| {
                (
                    nice(s.label.as_deref().unwrap_or(&s.id)),
                    format_reading(s.unit, s.value),
                )
            })
            .collect();
        if probes.is_empty() && others.is_empty() {
            return;
        }
        card(
            ui,
            None,
            |ui| {
                ui.label(legend("sensors"));
            },
            |ui| {
                if !probes.is_empty() {
                    let items: Vec<(&str, String, Color32)> =
                        probes.iter().map(|(l, v, t)| (l.as_str(), v.clone(), *t)).collect();
                    readouts(ui, &items);
                }
                for (label, value) in others {
                    reading(ui, &label, value);
                }
            },
        );
    }

    fn controls_card(&mut self, ui: &mut egui::Ui, status: &BatteryStatus) {
        let switches: Vec<(String, String, bool, bool)> = status
            .switches
            .iter()
            .map(|s| {
                let allowed = match s.id.as_str() {
                    "charging" => self.caps.contains(Capabilities::TOGGLE_CHARGE),
                    "discharging" => self.caps.contains(Capabilities::TOGGLE_DISCHARGE),
                    "balancer" => self.caps.contains(Capabilities::TOGGLE_BALANCER),
                    _ => self.caps.is_controllable(),
                };
                (
                    s.id.clone(),
                    s.label.clone().unwrap_or_else(|| nice(&s.id)),
                    self.shown(&s.id, s.on),
                    allowed,
                )
            })
            .collect();
        if switches.is_empty() && status.ports.is_empty() {
            return;
        }
        let ports: Vec<_> = status
            .ports
            .iter()
            .map(|p| {
                let mut p = p.clone();
                p.on = p.on.map(|on| self.shown(&p.id, on));
                p
            })
            .collect();
        let mut sends: Vec<(String, bool)> = Vec::new();
        card(
            ui,
            None,
            |ui| {
                ui.label(legend("outputs"));
            },
            |ui| {
                if !switches.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for (id, label, on, allowed) in &switches {
                            let r = ui.add_enabled_ui(*allowed, |ui| toggle(ui, label, *on)).inner;
                            if r.clicked() {
                                sends.push((id.clone(), !on));
                            }
                        }
                    });
                }
                for port in &ports {
                    ui.horizontal(|ui| {
                        let label = port.label.clone().unwrap_or_else(|| nice(&port.id));
                        let mut line = Line::new().legend(&label).column(ui, LABEL_W);
                        match port.watts {
                            Some(w) => line = line.measured(format!("{w:.0} W")),
                            None => line = line.note("-"),
                        }
                        line.show(ui);
                        if let Some(on) = port.on {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if port.settable {
                                    if toggle(ui, if on { "on" } else { "off" }, on).clicked() {
                                        sends.push((port.id.clone(), !on));
                                    }
                                } else {
                                    chip(ui, if on { "on" } else { "off" }, theme::TRACE, on);
                                }
                            });
                        }
                    });
                }
            },
        );
        for (id, on) in sends {
            self.flip(&id, on);
        }
    }

    fn settings_card(&mut self, ui: &mut egui::Ui, status: &BatteryStatus) {
        if status.settings.is_empty() {
            return;
        }
        let settings = status.settings.clone();
        let may_write = self.caps.contains(Capabilities::WRITE_SETTINGS);
        let mut sends: Vec<Control> = Vec::new();
        let mut toggled: Vec<(String, bool)> = Vec::new();
        let flips = self.pending.clone();
        let mut notices: Vec<String> = Vec::new();
        let mut edits = std::mem::take(&mut self.edits);
        let mut submitted = std::mem::take(&mut self.submitted);
        card(
            ui,
            None,
            |ui| {
                ui.label(legend("settings"));
                if !may_write {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(legend("read only"));
                    });
                }
            },
            |ui| {
                for setting in &settings {
                    setting_row(
                        ui,
                        setting,
                        may_write && setting.writable,
                        &mut edits,
                        &mut submitted,
                        &mut sends,
                        &mut toggled,
                        &flips,
                        &mut notices,
                    );
                }
            },
        );
        self.edits = edits;
        self.submitted = submitted;
        for send in sends {
            self.control(send);
        }
        for (id, on) in toggled {
            self.flip(&id, on);
        }
        if let Some(text) = notices.pop() {
            self.notice = Some((text, Instant::now()));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn setting_row(
    ui: &mut egui::Ui,
    setting: &Setting,
    writable: bool,
    edits: &mut HashMap<String, String>,
    submitted: &mut HashSet<String>,
    sends: &mut Vec<Control>,
    toggled: &mut Vec<(String, bool)>,
    flips: &HashMap<String, (bool, Instant)>,
    notices: &mut Vec<String>,
) {
    let label = setting.label.clone().unwrap_or_else(|| nice(&setting.id));
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(LABEL_W, 20.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                Line::new().legend(&label).elided(ui);
            },
        );
        let current = setting_text(setting);
        match (&setting.kind, &setting.value) {
            (SettingKind::Bool, SettingValue::Bool(on)) if writable => {
                let on = flips.get(&setting.id).map_or(*on, |(want, _)| *want);
                if toggle(ui, if on { "on" } else { "off" }, on).clicked() {
                    toggled.push((setting.id.clone(), !on));
                }
            }
            (SettingKind::Enum { options }, _) if writable => {
                let mut picked = edits.get(&setting.id).cloned().unwrap_or_else(|| {
                    match &setting.value {
                        SettingValue::Text(t) => t.clone(),
                        other => setting_value_text(other),
                    }
                });
                let before = picked.clone();
                ui.scope(|ui| {
                    ui.set_max_width((ui.available_width() - 16.0).max(80.0));
                    choice(
                        ui,
                        ("setting", &setting.id),
                        &mut picked,
                        options.iter().map(|o| (o.value.clone(), o.label.clone())),
                    );
                });
                if picked != before {
                    edits.insert(setting.id.clone(), picked.clone());
                    submitted.insert(setting.id.clone());
                    sends.push(Control::Set { id: setting.id.clone(), value: picked });
                }
            }
            (kind, _) if writable => {
                let mut text = edits.get(&setting.id).cloned().unwrap_or_else(|| current.clone());
                let unit = match kind {
                    SettingKind::Number { unit: Some(u), .. } => u.symbol(),
                    _ => "",
                };
                let reserve = if unit.is_empty() { 14.0 } else { 52.0 };
                let r = egui_bench::prelude::field_then(ui, &mut text, "", reserve, |ui| {
                    if !unit.is_empty() {
                        ui.label(legend(unit));
                    }
                });
                if r.changed() {
                    edits.insert(setting.id.clone(), text.clone());
                }
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if enter && edits.contains_key(&setting.id) {
                    match check_number(kind, &text) {
                        Ok(()) => {
                            submitted.insert(setting.id.clone());
                            sends.push(Control::Set { id: setting.id.clone(), value: text });
                        }
                        Err(why) => {
                            edits.remove(&setting.id);
                            notices.push(format!("{label}: {why}"));
                        }
                    }
                } else if r.lost_focus() {
                    edits.remove(&setting.id);
                }
            }
            _ => {
                let unit = match &setting.kind {
                    SettingKind::Number { unit: Some(u), .. } => format!(" {}", u.symbol()),
                    _ => String::new(),
                };
                Line::new().value(format!("{current}{unit}")).elided(ui);
            }
        }
    });
}

fn clipped(
    ui: &egui::Ui,
    text: &str,
    font: egui::FontId,
    colour: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, colour);
    job.wrap = egui::text::TextWrapping {
        max_width: width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('\u{2026}'),
    };
    ui.painter().layout_job(job)
}

fn device_row(ui: &mut egui::Ui, row: &Row, class: &str, on: bool) -> egui::Response {
    const H: f32 = 54.0;
    const PAD: f32 = 10.0;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), H), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if on {
            theme::READOUT.gamma_multiply(0.12)
        } else if resp.hovered() {
            theme::BAND
        } else {
            theme::WELL
        };
        let edge = if on { theme::READOUT } else { theme::ETCH };
        let p = ui.painter();
        p.rect_filled(rect, 2.0, fill);
        p.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, edge), egui::StrokeKind::Inside);
        if on {
            p.rect_filled(
                egui::Rect::from_min_size(rect.min, egui::vec2(3.0, H)),
                0.0,
                theme::READOUT,
            );
        }
        let w = rect.width() - 2.0 * PAD;
        let x = rect.left() + PAD;
        let title = clipped(ui, &row.label, theme::figure(12.5), theme::VALUE, w);
        let kind = clipped(
            ui,
            &format!("{} / {class}", row.backend).to_uppercase(),
            theme::legend_font(10.0),
            if on { theme::READOUT } else { theme::LEGEND },
            w,
        );
        let id = clipped(ui, &row.id, theme::mono(9.5), theme::LEGEND.gamma_multiply(0.8), w);
        let p = ui.painter();
        p.galley(egui::pos2(x, rect.top() + 7.0), title, theme::VALUE);
        p.galley(egui::pos2(x, rect.top() + 25.0), kind, theme::LEGEND);
        p.galley(egui::pos2(x, rect.top() + 39.0), id, theme::LEGEND);
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(&row.id)
}

fn live_bool(status: &BatteryStatus, id: &str) -> Option<bool> {
    status
        .switches
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.on)
        .or_else(|| status.ports.iter().find(|p| p.id == id).and_then(|p| p.on))
        .or_else(|| match status.setting(id).map(|s| &s.value) {
            Some(SettingValue::Bool(b)) => Some(*b),
            _ => None,
        })
}

fn check_number(kind: &SettingKind, text: &str) -> Result<(), String> {
    let SettingKind::Number { min, max, .. } = kind else { return Ok(()) };
    let value: f64 = text.trim().parse().map_err(|_| format!("{text:?} is not a number"))?;
    if let Some(min) = min.filter(|m| value < *m) {
        return Err(format!("{value} is below {min}"));
    }
    if let Some(max) = max.filter(|m| value > *m) {
        return Err(format!("{value} is above {max}"));
    }
    Ok(())
}

fn setting_text(setting: &Setting) -> String {
    match (&setting.kind, &setting.value) {
        (SettingKind::Enum { options }, SettingValue::Text(t)) => options
            .iter()
            .find(|o| &o.value == t)
            .map(|o| o.label.clone())
            .unwrap_or_else(|| t.clone()),
        (_, value) => setting_value_text(value),
    }
}

fn setting_value_text(value: &SettingValue) -> String {
    match value {
        SettingValue::Bool(b) => if *b { "on" } else { "off" }.into(),
        SettingValue::Number(n) => trim_number(*n),
        SettingValue::Text(t) => t.clone(),
    }
}

fn trim_number(n: f64) -> String {
    let s = format!("{n:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn format_reading(unit: Unit, value: f64) -> String {
    match unit {
        Unit::Count => format!("{value:.0}"),
        Unit::Percent | Unit::Watt | Unit::Second | Unit::Minute => {
            format!("{value:.0} {}", unit.symbol())
        }
        Unit::Hour => format!("{value:.1} h"),
        Unit::Volt => format!("{value:.2} V"),
        _ => format!("{value:.2} {}", unit.symbol()),
    }
}

fn nice(id: &str) -> String {
    let id = id.strip_prefix(battery_control::TEMP_PREFIX).unwrap_or(id);
    let mut s = id.replace(['_', '.'], " ");
    if let Some(first) = s.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    s
}

fn minutes(seconds: f32) -> String {
    if seconds < 90.0 {
        format!("{seconds:.0} s")
    } else {
        format!("{:.0} min", seconds / 60.0)
    }
}

impl eframe::App for App {
    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        self.drain();
        ctx.request_repaint_after(Duration::from_millis(500));

        egui::Panel::top("head")
            .frame(egui::Frame::NONE.fill(theme::WELL).inner_margin(8))
            .show(root, |ui| self.header(ui));

        egui::Panel::left("devices")
            .exact_size(SIDE_W)
            .resizable(false)
            .frame(egui::Frame::NONE.fill(theme::CHASSIS).inner_margin(8))
            .show(root, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    self.devices_card(ui);
                    self.identity_card(ui);
                    self.pairing_card(ui);
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::CHASSIS).inner_margin(8))
            .show(root, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                    match (&self.link, self.status.clone()) {
                        (Link::Live, Some(status)) => {
                            self.summary_card(ui, &status);
                            self.cells_card(ui, &status);
                            self.history_card(ui);
                            self.temperature_card(ui, &status);
                            self.controls_card(ui, &status);
                            self.settings_card(ui, &status);
                        }
                        (Link::Live, None) => {
                            ui.label(legend("waiting for the first reading"));
                        }
                        _ => {
                            ui.add_space(24.0);
                            ui.label(legend("no battery connected"));
                            note(
                                ui,
                                "Pick a device on the left and press connect. Bluetooth batteries \
                                 are found by scanning, USB serial ones by asking each port what \
                                 it is.",
                                theme::LEGEND,
                            );
                        }
                    }
                });
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.worker.close();
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("battery-control {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let demo = args.iter().any(|a| a == "--demo");
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1100.0, 780.0])
        .with_min_inner_size([760.0, 520.0])
        .with_title("battery control")
        .with_app_id("battery-control");
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "battery-control",
        options,
        Box::new(move |cc| {
            egui_bench::install(&cc.egui_ctx);
            App::new(cc.egui_ctx.clone(), demo)
                .map(|app| Box::new(app) as Box<dyn eframe::App>)
                .map_err(Into::into)
        }),
    )
}
