//! Pylontech US2000/US3000 **C-series text console** (RJ45 "Console" port,
//! real RS232, 115200 8N1). The C series answers the interactive CLI (`bat`,
//! `pwr`, `info`, `stat`) but rejects the `~20...` frame protocol that
//! [`super::pylontech_rs485`] speaks, so this is the only way in on those units.
//!
//! Unlike the frame backends this one reports per-cell balancing flags, SOH,
//! cycle count and the module barcode.

use crate::battery::Battery;
use crate::types::{BatteryStatus, CellInfo, Reading, Unit};
use crate::{Capabilities, DeviceInfo, Error, Result};
use async_trait::async_trait;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

const PROMPT: &[u8] = b"pylon>";

pub struct PylontechCli {
    port: SerialStream,
    info: DeviceInfo,
    identified: bool,
    rated_ah: Option<f64>,
    lifetime: Option<Lifetime>,
    lifetime_read: Option<std::time::Instant>,
}

/// Counters that only move over hours, so they are re-read on an interval
/// rather than on every status poll.
#[derive(Debug, Clone, Copy)]
struct Lifetime {
    soh: Option<f64>,
    cycles: Option<f64>,
}

const LIFETIME_REFRESH: Duration = Duration::from_secs(300);

#[derive(Debug, Default, Clone)]
pub struct CellRow {
    pub millivolts: u16,
    pub balancing: bool,
}

#[derive(Debug, Default, Clone)]
pub struct BatFrame {
    pub cells: Vec<CellRow>,
    /// Anything the BMS flagged as other than normal, per cell and column.
    pub alarms: Vec<String>,
    pub current_ma: i32,
    pub temp_milli_c: i32,
    pub soc_percent: u8,
    pub state: String,
}

impl PylontechCli {
    pub async fn open_serial(path: &str, baud: u32) -> Result<Self> {
        let mut port = tokio_serial::new(path, baud)
            .timeout(Duration::from_millis(100))
            .open_native_async()
            .map_err(|e| Error::Transport(e.to_string()))?;
        port.write_all(b"\n")
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut flush = [0u8; 4096];
        let _ = tokio::time::timeout(Duration::from_millis(200), port.read(&mut flush)).await;
        Ok(Self {
            port,
            info: DeviceInfo {
                backend: "pylontech-cli".into(),
                manufacturer: Some("Pylontech".into()),
                ..Default::default()
            },
            identified: false,
            rated_ah: None,
            lifetime: None,
            lifetime_read: None,
        })
    }

    pub async fn command(&mut self, cmd: &str) -> Result<String> {
        self.port
            .write_all(format!("{cmd}\n").as_bytes())
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                break;
            }
            match tokio::time::timeout(left.min(Duration::from_millis(400)), self.port.read(&mut chunk)).await {
                Ok(Ok(0)) | Err(_) if !buf.is_empty() => break,
                Ok(Ok(n)) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.ends_with(PROMPT) || buf.trim_ascii_end().ends_with(PROMPT) {
                        break;
                    }
                }
                Ok(Err(e)) => return Err(Error::Transport(e.to_string())),
                Err(_) => {}
            }
        }
        Ok(String::from_utf8_lossy(&buf).replace('\r', ""))
    }

    pub async fn bat(&mut self) -> Result<BatFrame> {
        let text = self.command("bat").await?;
        parse_bat(&text).ok_or_else(|| Error::Transport("no cell rows in `bat` output".into()))
    }

    async fn identify(&mut self) -> Result<()> {
        let info = self.command("info").await?;
        for line in info.lines() {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim().to_string());
            match k {
                "Barcode" => self.info.serial = Some(v),
                "Device name" => self.info.model = Some(v),
                "Specification" => {
                    self.rated_ah = rated_ah(&v);
                    if self.info.model.is_none() {
                        self.info.model = Some(format!("Pylontech {v}"));
                    }
                }
                "Main Soft version" => self.info.firmware = Some(v),
                "Board version" => self.info.hardware = Some(v),
                "Manufacturer" => self.info.manufacturer = Some(v),
                _ => {}
            }
        }
        self.identified = true;
        Ok(())
    }
}

/// Pull the amp-hour rating out of a specification string like `48V/50AH`.
fn rated_ah(spec: &str) -> Option<f64> {
    let up = spec.to_ascii_uppercase();
    let ah = up.find("AH")?;
    let digits: String = up[..ah]
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    digits.chars().rev().collect::<String>().parse().ok()
}

pub fn parse_bat(text: &str) -> Option<BatFrame> {
    let mut f = BatFrame::default();
    for line in text.lines() {
        let col: Vec<&str> = line.split_whitespace().collect();
        if col.len() < 9 {
            continue;
        }
        let (Ok(_idx), Ok(mv)) = (col[0].parse::<u8>(), col[1].parse::<u16>()) else {
            continue;
        };
        // Columns 4..8 are the base, voltage, current and temperature states.
        // Anything but "Normal" is the BMS telling you something.
        for (label, state) in [
            ("state", col[4]),
            ("voltage", col[5]),
            ("current", col[6]),
            ("temperature", col[7]),
        ] {
            let normal = state.eq_ignore_ascii_case("normal")
                || state.eq_ignore_ascii_case("charge")
                || state.eq_ignore_ascii_case("dischg")
                || state.eq_ignore_ascii_case("idle")
                || state.eq_ignore_ascii_case("absent");
            if !normal {
                f.alarms.push(format!("cell {} {label}: {state}", f.cells.len()));
            }
        }
        f.cells.push(CellRow {
            millivolts: mv,
            balancing: col[col.len() - 1] == "Y",
        });
        f.current_ma = col[2].parse().unwrap_or(f.current_ma);
        f.temp_milli_c = col[3].parse().unwrap_or(f.temp_milli_c);
        f.state = col[4].to_string();
        f.soc_percent = col[8].trim_end_matches('%').parse().unwrap_or(f.soc_percent);
    }
    (!f.cells.is_empty()).then_some(f)
}

fn to_status(f: &BatFrame) -> BatteryStatus {
    let mut s = BatteryStatus::default();
    let pack_v = f.cells.iter().map(|c| c.millivolts as f64).sum::<f64>() / 1000.0;
    let current = f.current_ma as f64 / 1000.0;
    let power = pack_v * current;
    s.set(Reading::Soc, Some(f.soc_percent as f64))
        .set(Reading::Voltage, Some(pack_v))
        .set(Reading::Current, Some(current))
        .set(Reading::PowerIn, (power > 0.0).then_some(power))
        .set(Reading::PowerOut, (power < 0.0).then(|| power.abs()));
    s.set_labeled(
        "temp.pack",
        "Pack",
        f.temp_milli_c as f64 / 1000.0,
        Unit::Celsius,
    );
    s.alarms = f.alarms.clone();
    s.cells = f
        .cells
        .iter()
        .enumerate()
        .map(|(i, c)| CellInfo {
            index: i as u8,
            voltage: Some(c.millivolts as f32 / 1000.0),
            resistance: None,
            balancing: Some(c.balancing),
        })
        .collect();
    s
}

#[async_trait]
impl Battery for PylontechCli {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::READ_BASIC | Capabilities::READ_CELLS | Capabilities::READ_TEMPERATURE
    }

    async fn status(&mut self) -> Result<BatteryStatus> {
        if !self.identified {
            self.identify().await?;
        }
        let frame = self.bat().await?;
        let mut status = to_status(&frame);
        let stale = self
            .lifetime_read
            .map(|t| t.elapsed() >= LIFETIME_REFRESH)
            .unwrap_or(true);
        if stale && let Ok(stat) = self.command("stat").await {
            let mut life = Lifetime {
                soh: None,
                cycles: None,
            };
            for line in stat.lines() {
                let Some((k, v)) = line.split_once(':') else {
                    continue;
                };
                let v = v.trim().parse::<f64>().ok();
                match k.trim() {
                    "SOH" => life.soh = v,
                    "CYCLE Times" => life.cycles = v,
                    _ => {}
                }
            }
            self.lifetime = Some(life);
            self.lifetime_read = Some(std::time::Instant::now());
        }
        if let Some(life) = self.lifetime {
            status.set(Reading::Soh, life.soh);
            status.set(Reading::Cycles, life.cycles);
        }
        status.set(Reading::CapacityFullAh, self.rated_ah);
        Ok(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "bat\n@\nBattery  Volt     Curr     Tempr    Base State   Volt. State  Curr. State  Temp. State  SOC          Coulomb      BAL         \n0        3331     5852     23000    Charge       Normal       Normal       Normal       14%          6182 mAH      N\n1        3332     5852     23000    Charge       Normal       Normal       Normal       14%          6182 mAH      N\n5        3363     5852     23000    Charge       Normal       Normal       Normal       14%          6182 mAH      Y\nCommand completed successfully\n$$\npylon>";

    #[test]
    fn parses_cells_and_balancing() {
        let f = parse_bat(SAMPLE).expect("frame");
        assert_eq!(f.cells.len(), 3);
        assert_eq!(f.cells[0].millivolts, 3331);
        assert!(!f.cells[0].balancing);
        assert!(f.cells[2].balancing);
        assert_eq!(f.current_ma, 5852);
        assert_eq!(f.soc_percent, 14);
        assert_eq!(f.state, "Charge");
    }

    #[test]
    fn reads_rating_from_specification() {
        assert_eq!(rated_ah("48V/50AH"), Some(50.0));
        assert_eq!(rated_ah("24V/100Ah"), Some(100.0));
        assert_eq!(rated_ah("48V"), None);
    }

    #[test]
    fn flags_any_cell_state_that_is_not_normal() {
        let bad = SAMPLE.replace(
            "0        3331     5852     23000    Charge       Normal       Normal       Normal",
            "0        3331     5852     23000    Charge       High         Normal       Normal",
        );
        let f = parse_bat(&bad).expect("frame");
        assert_eq!(f.alarms, vec!["cell 0 voltage: High"]);
        assert!(parse_bat(SAMPLE).unwrap().alarms.is_empty());
    }

    #[test]
    fn maps_status_from_cells() {
        let s = to_status(&parse_bat(SAMPLE).unwrap());
        assert_eq!(s.cells.len(), 3);
        assert_eq!(s.cells[2].balancing, Some(true));
        assert!((s.get(Reading::Voltage).unwrap() - 10.026).abs() < 1e-9);
    }
}
