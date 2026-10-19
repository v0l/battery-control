use async_trait::async_trait;
use battery_control::{
    Battery, BatteryStatus, Capabilities, CellInfo, Command, DeviceInfo, Error, PortDirection,
    PortInfo, Reading, Result, Setting, SettingKind, SettingValue, Unit,
};
use std::time::Instant;

const CELLS: usize = 16;
const OFFSET_MV: [f64; CELLS] =
    [1.0, 11.0, 14.0, -8.0, -4.0, -30.0, 9.0, -4.0, -3.0, -4.0, 15.0, -16.0, 0.0, 9.0, 25.0, 3.0];

pub struct DemoBattery {
    info: DeviceInfo,
    started: Instant,
    charging: bool,
    discharging: bool,
    balancer: bool,
    ac: bool,
    charge_limit: f64,
    beeper: String,
    name: String,
}

impl DemoBattery {
    pub fn new() -> Self {
        Self {
            info: DeviceInfo {
                backend: "demo".into(),
                manufacturer: Some("Bench".into()),
                model: Some("Demo 16S LiFePO4".into()),
                serial: Some("DEMO-0001".into()),
                firmware: Some("1.0.0".into()),
                hardware: None,
            },
            started: Instant::now(),
            charging: true,
            discharging: true,
            balancer: true,
            ac: false,
            charge_limit: 90.0,
            beeper: "alarms".into(),
            name: "Rack 1".into(),
        }
    }
}

#[async_trait]
impl Battery for DemoBattery {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::READ_BASIC
            | Capabilities::READ_CELLS
            | Capabilities::READ_PORTS
            | Capabilities::READ_TEMPERATURE
            | Capabilities::READ_LIMITS
            | Capabilities::READ_ALARMS
            | Capabilities::TOGGLE_CHARGE
            | Capabilities::TOGGLE_DISCHARGE
            | Capabilities::TOGGLE_BALANCER
            | Capabilities::WRITE_SETTINGS
    }

    async fn status(&mut self) -> Result<BatteryStatus> {
        let minute = self.started.elapsed().as_secs_f64() / 60.0 + 40.0;
        let taper = (-minute / 55.0).exp();
        let current = if self.charging { 0.2 + 28.0 * taper } else { 0.0 };
        let climb = 3452.0 - 34.0 * taper;
        let cell_mv: Vec<f64> = OFFSET_MV
            .iter()
            .enumerate()
            .map(|(i, off)| {
                let wobble = ((minute / 7.0) + i as f64).sin() * 1.5;
                (climb + off * (1.0 + 0.6 * (1.0 - taper)) + wobble).round()
            })
            .collect();
        let highest = cell_mv.iter().copied().fold(f64::MIN, f64::max);
        let pack_v = cell_mv.iter().sum::<f64>() / 1000.0;
        let soc = (100.0 - 62.0 * taper).min(100.0);

        let mut s = BatteryStatus::default();
        s.set(Reading::Soc, Some(soc))
            .set(Reading::Soh, Some(98.0))
            .set(Reading::Voltage, Some(pack_v))
            .set(Reading::Current, Some(current))
            .set(Reading::PowerIn, Some(pack_v * current))
            .set(Reading::CapacityRemainingAh, Some(soc * 1.2))
            .set(Reading::CapacityFullAh, Some(120.0))
            .set(Reading::Cycles, Some(212.0))
            .set(Reading::ChargeCurrentLimitA, Some(50.0))
            .set(Reading::DischargeCurrentLimitA, Some(100.0))
            .set(Reading::SocLimitMax, Some(self.charge_limit))
            .set_labeled("temp.mosfet", "MOSFET", 31.0 + 6.0 * taper, Unit::Celsius)
            .set_labeled("temp.cells", "Cells", 24.5 + 3.0 * taper, Unit::Celsius)
            .set_custom_switch("charging", Some("Charging"), Some(self.charging))
            .set_custom_switch("discharging", Some("Discharging"), Some(self.discharging))
            .set_custom_switch("balancer", Some("Balancer"), Some(self.balancer));
        s.cells = cell_mv
            .iter()
            .enumerate()
            .map(|(i, mv)| CellInfo {
                index: i as u8,
                voltage: Some((*mv / 1000.0) as f32),
                resistance: None,
                balancing: Some(self.balancer && *mv >= highest - 18.0),
            })
            .collect();
        s.ports.push(PortInfo {
            id: "ac".into(),
            label: Some("AC inverter".into()),
            direction: Some(PortDirection::Out),
            on: Some(self.ac),
            watts: Some(if self.ac { 640.0 } else { 0.0 }),
            settable: true,
        });
        s.ports.push(PortInfo {
            id: "solar".into(),
            label: Some("Solar input".into()),
            direction: Some(PortDirection::In),
            on: Some(self.charging),
            watts: Some((pack_v * current) as f32),
            settable: false,
        });
        s.add_setting(Setting {
            id: "charge_limit".into(),
            label: Some("Charge limit".into()),
            value: SettingValue::Number(self.charge_limit),
            kind: SettingKind::Number {
                min: Some(50.0),
                max: Some(100.0),
                step: Some(1.0),
                unit: Some(Unit::Percent),
            },
            writable: true,
        })
        .add_setting(Setting {
            id: "beeper".into(),
            label: Some("Beeper".into()),
            value: SettingValue::Text(self.beeper.clone()),
            kind: SettingKind::enum_of([("off", "Off"), ("alarms", "Alarms only"), ("all", "All")]),
            writable: true,
        })
        .add_setting(Setting {
            id: "name".into(),
            label: Some("Name".into()),
            value: SettingValue::Text(self.name.clone()),
            kind: SettingKind::Text,
            writable: true,
        })
        .add_setting(Setting {
            id: "cell_ovp".into(),
            label: Some("Cell over voltage".into()),
            value: SettingValue::Number(3.65),
            kind: SettingKind::Number { min: None, max: None, step: None, unit: Some(Unit::Volt) },
            writable: false,
        });
        if highest >= 3500.0 {
            s.alarms.push("cell high".into());
        }
        Ok(s)
    }

    async fn execute(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Toggle { id, on } => {
                match id.as_str() {
                    "charging" => self.charging = on,
                    "discharging" => self.discharging = on,
                    "balancer" => self.balancer = on,
                    "ac" => self.ac = on,
                    _ => return Err(Error::Unsupported),
                }
                Ok(())
            }
            Command::Set { id, value } => match id.as_str() {
                "charge_limit" => {
                    self.charge_limit = value
                        .parse::<f64>()
                        .map_err(|_| Error::InvalidArgument(format!("{value} is not a number")))?
                        .clamp(50.0, 100.0);
                    Ok(())
                }
                "beeper" => {
                    self.beeper = value;
                    Ok(())
                }
                "name" => {
                    self.name = value;
                    Ok(())
                }
                _ => Err(Error::Unsupported),
            },
        }
    }
}
