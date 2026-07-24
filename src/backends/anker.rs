//! Adapter for [`anker_solix`] — Anker SOLIX portable power stations over BLE.

use crate::battery::Battery;
use crate::types::{
    BatteryStatus, Command, PortDirection, PortInfo, Reading, Setting, SettingKind, SettingValue,
    Unit,
};
use crate::{Capabilities, DeviceInfo, Error, Result};
use anker_solix::{Brightness, Device, PortStatus as AnkerPort, Telemetry};
use async_trait::async_trait;
use std::time::Duration;

/// A SOLIX power station exposed through the unified [`Battery`] trait.
pub struct AnkerBattery {
    device: Device,
    info: DeviceInfo,
    /// Last-seen SOC limits, cached so a single-field write can supply the pair
    /// that [`anker_solix::Device::set_soc_limits`] requires.
    soc_max: Option<u8>,
    soc_min: Option<u8>,
}

impl AnkerBattery {
    /// Discover and connect to a station by name substring or MAC.
    pub async fn connect(target: &str, scan_secs: u64) -> Result<Self> {
        let device = Device::find_and_connect(target, scan_secs)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let info = DeviceInfo {
            backend: "anker".into(),
            model: Some(device.name().to_string()),
            ..Default::default()
        };
        Ok(Self { device, info, soc_max: None, soc_min: None })
    }

    /// Wrap an already-connected [`anker_solix::Device`].
    pub fn from_device(device: Device) -> Self {
        let info = DeviceInfo {
            backend: "anker".into(),
            model: Some(device.name().to_string()),
            ..Default::default()
        };
        Self { device, info, soc_max: None, soc_min: None }
    }

    /// Remember the SOC limits from a fresh telemetry frame.
    fn cache_soc(&mut self, t: &Telemetry) {
        if let Some(v) = t.max_battery_percentage {
            self.soc_max = Some(v.clamp(0, 100) as u8);
        }
        if let Some(v) = t.min_battery_percentage {
            self.soc_min = Some(v.clamp(0, 100) as u8);
        }
    }

    /// Dispatch a numeric/enum `Command::Set` to the matching crate setter.
    async fn set_value(&mut self, id: &str, value: &str) -> Result<()> {
        let num: f64 = value
            .trim()
            .parse()
            .map_err(|_| Error::InvalidArgument(format!("'{value}' is not a number")))?;
        let dev = &mut self.device;
        let res = match id {
            "soc_limit_max" => {
                let max = num.round().clamp(0.0, 100.0) as u8;
                let min = self.soc_min.ok_or_else(|| {
                    Error::InvalidArgument("read status once before setting SOC limits".into())
                })?;
                self.soc_max = Some(max);
                dev.set_soc_limits(max, min).await
            }
            "soc_limit_min" => {
                let min = num.round().clamp(0.0, 100.0) as u8;
                let max = self.soc_max.ok_or_else(|| {
                    Error::InvalidArgument("read status once before setting SOC limits".into())
                })?;
                self.soc_min = Some(min);
                dev.set_soc_limits(max, min).await
            }
            "ac_charge_power" => dev.set_ac_charge_power(num.round().max(0.0) as u16).await,
            "ac_frequency" => {
                let hz = num.round() as u8;
                if hz != 50 && hz != 60 {
                    return Err(Error::InvalidArgument("AC frequency must be 50 or 60".into()));
                }
                dev.set_ac_frequency(hz).await
            }
            "standby_timeout" => dev.set_standby_timeout(num.round().max(0.0) as u16).await,
            "screen_timeout" => dev.set_screen_timeout(num.round().max(0.0) as u16).await,
            "display_brightness" => dev.set_brightness(num.round().clamp(0.0, 3.0) as u8).await,
            other => {
                return Err(Error::InvalidArgument(format!(
                    "'{other}' is not a settable value on this device"
                )))
            }
        };
        res.map_err(|e| Error::Transport(e.to_string()))
    }
}

fn number(unit: Option<Unit>, min: f64, max: f64, step: f64) -> SettingKind {
    SettingKind::Number { min: Some(min), max: Some(max), step: Some(step), unit }
}

fn num_setting(id: &str, label: &str, value: i64, kind: SettingKind) -> Setting {
    Setting {
        id: id.into(),
        label: Some(label.into()),
        value: SettingValue::Number(value as f64),
        kind,
        writable: true,
    }
}

fn bool_setting(id: &str, label: &str, on: bool) -> Setting {
    Setting {
        id: id.into(),
        label: Some(label.into()),
        value: SettingValue::Bool(on),
        kind: SettingKind::Bool,
        writable: true,
    }
}

/// Build the writable settings list from a telemetry frame (gen-2 only; gen-1
/// leaves these `None`, so nothing is added).
fn settings(t: &Telemetry) -> Vec<Setting> {
    let mut out = Vec::new();
    if let Some(v) = t.max_battery_percentage {
        out.push(num_setting("soc_limit_max", "Charge limit", v,
            number(Some(Unit::Percent), 0.0, 100.0, 1.0)));
    }
    if let Some(v) = t.min_battery_percentage {
        out.push(num_setting("soc_limit_min", "Discharge limit", v,
            number(Some(Unit::Percent), 0.0, 100.0, 1.0)));
    }
    if let Some(v) = t.ac_charge_power_w {
        out.push(num_setting("ac_charge_power", "AC charge power", v,
            number(Some(Unit::Watt), 100.0, 1300.0, 10.0)));
    }
    if let Some(v) = t.ac_frequency_hz {
        out.push(Setting {
            id: "ac_frequency".into(),
            label: Some("AC frequency".into()),
            value: SettingValue::Number(v as f64),
            kind: SettingKind::Enum { options: vec!["50".into(), "60".into()] },
            writable: true,
        });
    }
    if let Some(v) = t.standby_timeout_min {
        out.push(num_setting("standby_timeout", "Standby timeout", v,
            number(Some(Unit::Minute), 0.0, 1440.0, 1.0)));
    }
    if let Some(v) = t.screen_timeout_min {
        out.push(num_setting("screen_timeout", "Screen timeout", v,
            number(Some(Unit::Minute), 0.0, 1440.0, 1.0)));
    }
    if let Some(v) = t.display_brightness {
        out.push(num_setting("display_brightness", "Display brightness", v,
            number(None, 0.0, 3.0, 1.0)));
    }
    if let Some(on) = t.smart_ac {
        out.push(bool_setting("smart_ac", "Smart AC", on));
    }
    if let Some(on) = t.car_saving {
        out.push(bool_setting("car_saving", "Car energy saving", on));
    }
    if let Some(on) = t.port_memory {
        out.push(bool_setting("port_memory", "Port memory", on));
    }
    if let Some(on) = t.screensaver {
        out.push(bool_setting("screensaver", "Clock screensaver", on));
    }
    out
}

fn port(id: &str, label: &str, p: &anker_solix::Port, settable: bool) -> PortInfo {
    let (on, direction) = match p.status {
        AnkerPort::Output => (Some(true), Some(PortDirection::Out)),
        AnkerPort::Input => (Some(true), Some(PortDirection::In)),
        AnkerPort::Off => (Some(false), None),
        AnkerPort::Unknown => (None, None),
    };
    PortInfo {
        id: id.to_string(),
        label: Some(label.to_string()),
        direction,
        on,
        watts: p.watts.map(|w| w as f32),
        settable,
    }
}

fn to_status(t: &Telemetry) -> BatteryStatus {
    // Only the AC and DC outputs accept on/off control; solar input and the
    // USB ports are monitor-only.
    let mut ports = vec![
        port("ac", "AC", &t.ac, true),
        port("dc", "DC (12V)", &t.dc, true),
        port("solar", "Solar", &t.solar, false),
        port("usb_c1", "USB-C 1", &t.usb_c1, false),
        port("usb_c2", "USB-C 2", &t.usb_c2, false),
        port("usb_c3", "USB-C 3", &t.usb_c3, false),
        port("usb_a1", "USB-A 1", &t.usb_a1, false),
    ];
    ports.retain(|p| p.on.is_some() || p.watts.is_some());

    let mut s = BatteryStatus::default();
    s.set(Reading::Soc, t.battery_percentage.map(|v| v as f64))
        .set(Reading::Soh, t.battery_health.map(|v| v as f64))
        .set(Reading::PowerIn, t.ac_power_in.map(|v| v as f64))
        .set(Reading::PowerOut, t.power_out.map(|v| v as f64))
        .set(Reading::SocLimitMax, t.max_battery_percentage.map(|v| v as f64))
        .set(Reading::SocLimitMin, t.min_battery_percentage.map(|v| v as f64))
        .set(Reading::TimeRemainingH, t.time_remaining_hours);
    if let Some(c) = t.temperature_c {
        s.set_labeled("temp.battery", "Battery", c as f64, Unit::Celsius);
    }
    s.ports = ports;
    s.settings = settings(t);
    s
}

#[async_trait]
impl Battery for AnkerBattery {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::READ_BASIC
            | Capabilities::READ_PORTS
            | Capabilities::READ_TEMPERATURE
            | Capabilities::WRITE_SETTINGS
    }

    async fn status(&mut self) -> Result<BatteryStatus> {
        let t = self
            .device
            .next_telemetry(Duration::from_secs(12))
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        // Enrich static info from the first snapshot.
        if self.info.serial.is_none() {
            self.info.serial = t.serial_number.clone();
        }
        self.cache_soc(&t);
        Ok(to_status(&t))
    }

    async fn execute(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Toggle { id, on } => {
                match id.as_str() {
                    "ac" => self.device.set_ac(on).await,
                    "dc" => self.device.set_dc(on).await,
                    // Light bar / display are gen-1 only; the device returns
                    // UnsupportedModel on gen-2 until the codes are known.
                    "display" => self.device.set_display(on).await,
                    "light" => {
                        self.device
                            .set_light(if on { Brightness::High } else { Brightness::Off })
                            .await
                    }
                    "smart_ac" => self.device.set_smart_ac(on).await,
                    "car_saving" => self.device.set_car_saving(on).await,
                    "port_memory" => self.device.set_port_memory(on).await,
                    "screensaver" => self.device.set_screensaver(on).await,
                    other => {
                        return Err(Error::InvalidArgument(format!(
                            "'{other}' is not controllable on this device"
                        )))
                    }
                }
                .map_err(|e| Error::Transport(e.to_string()))
            }
            Command::Set { id, value } if id == "light" => {
                let b = Brightness::parse(&value)
                    .ok_or_else(|| Error::InvalidArgument(format!("bad light mode: {value}")))?;
                self.device
                    .set_light(b)
                    .await
                    .map_err(|e| Error::Transport(e.to_string()))
            }
            Command::Set { id, value } => self.set_value(&id, &value).await,
        }
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.device.disconnect().await.map_err(|e| Error::Transport(e.to_string()))
    }

    /// Real-time state: the SOLIX pushes a full frame over BLE notifications; we
    /// diff consecutive frames and emit only the individual
    /// [`StatusUpdate`](crate::StatusUpdate)s that changed.
    fn has_stream(&self) -> bool {
        true
    }

    fn stream(&mut self) -> Option<crate::battery::StatusStream<'_>> {
        use std::collections::VecDeque;
        type State<'a> = (
            &'a mut AnkerBattery,
            Option<BatteryStatus>,
            VecDeque<crate::StatusUpdate>,
            bool,
        );
        let init: State = (self, None, VecDeque::new(), false);
        let stream = futures_util::stream::unfold(
            init,
            |(this, mut prev, mut queue, ended): State| async move {
                loop {
                    // Drain buffered updates from the last frame first.
                    if let Some(u) = queue.pop_front() {
                        return Some((Ok(u), (this, prev, queue, ended)));
                    }
                    if ended {
                        return None;
                    }
                    match this.device.next_telemetry(Duration::from_secs(30)).await {
                        Ok(t) => {
                            if this.info.serial.is_none() {
                                this.info.serial = t.serial_number.clone();
                            }
                            this.cache_soc(&t);
                            let status = to_status(&t);
                            queue.extend(status.diff(prev.as_ref()));
                            prev = Some(status);
                            // Loop to emit the first queued update (if any).
                        }
                        // Surface the error once, then end the stream so callers
                        // can reconnect rather than spin on a dead channel.
                        Err(e) => {
                            return Some((
                                Err(Error::Transport(e.to_string())),
                                (this, prev, queue, true),
                            ));
                        }
                    }
                }
            },
        );
        Some(Box::pin(stream))
    }
}
