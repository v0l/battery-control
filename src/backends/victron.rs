//! Adapter for [`victron_ble`] — Victron devices broadcasting "Instant Readout"
//! data over BLE. Read-only battery monitors (BMV/SmartShunt).
//!
//! Requires the device's name and its BLE encryption key (VictronConnect →
//! Settings → Product info → Encryption data).

use crate::battery::Battery;
use crate::types::{BatteryStatus, Reading, Unit};
use crate::{Capabilities, DeviceInfo, Error, Result};
use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use std::pin::Pin;
use std::time::Duration;
use victron_ble::{AuxInput, BatteryMonitorState, DeviceState};

type StateStream =
    Pin<Box<dyn Stream<Item = std::result::Result<DeviceState, victron_ble::Error>> + Send>>;

/// A Victron battery monitor exposed through the unified [`Battery`] trait.
pub struct VictronMonitor {
    stream: StateStream,
    info: DeviceInfo,
}

impl VictronMonitor {
    /// Open the Instant Readout broadcast stream for `name` using `key`
    /// (the raw encryption key bytes).
    pub fn open(name: impl Into<String>, key: Vec<u8>) -> Result<Self> {
        let name = name.into();
        let stream = victron_ble::open_stream(name.clone(), key)
            .map_err(|e| Error::Transport(format!("{e:?}")))?;
        Ok(Self {
            stream: Box::pin(stream),
            info: DeviceInfo {
                backend: "victron".into(),
                model: Some(name),
                ..Default::default()
            },
        })
    }
}

fn to_status(m: &BatteryMonitorState) -> BatteryStatus {
    let power = match (m.battery_voltage_v, m.battery_current_a) {
        (Some(v), Some(i)) => Some(v * i),
        _ => None,
    };
    let mut s = BatteryStatus::default();
    s.set(Reading::Soc, m.state_of_charge_pct.map(|v| v as f64))
        .set(Reading::Voltage, m.battery_voltage_v.map(|v| v as f64))
        .set(Reading::Current, m.battery_current_a.map(|v| v as f64))
        .set(Reading::PowerIn, power.filter(|p| *p > 0.0).map(|v| v as f64))
        .set(Reading::PowerOut, power.filter(|p| *p < 0.0).map(|v| v.abs() as f64))
        .set(Reading::TimeRemainingH, m.time_to_go_mins.map(|min| (min / 60.0) as f64));

    // Consumed Ah is reported negative; remaining is unknowable without a
    // configured capacity, so expose the consumed magnitude as its own sensor.
    if let Some(ah) = m.consumed_amp_hours_ah {
        s.set_labeled("consumed_ah", "Consumed", ah.abs() as f64, Unit::AmpHour);
    }
    // A temperature aux probe (SmartShunt) reports kelvin.
    if let AuxInput::TemperatureK(k) = m.aux_input {
        s.set_labeled("temp.battery", "Battery", (k - 273.15) as f64, Unit::Celsius);
    }
    if !m.alarm_reason.is_empty() {
        s.alarms = m
            .alarm_reason
            .iter_names()
            .map(|(name, _)| name.to_ascii_lowercase().replace('_', " "))
            .collect();
    }
    s
}

async fn next_monitor(stream: &mut StateStream) -> Option<BatteryStatus> {
    while let Some(item) = stream.next().await {
        if let Ok(DeviceState::BatteryMonitor(m)) = item {
            return Some(to_status(&m));
        }
    }
    None
}

#[async_trait]
impl Battery for VictronMonitor {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::READ_BASIC | Capabilities::READ_TEMPERATURE | Capabilities::READ_ALARMS
    }

    async fn status(&mut self) -> Result<BatteryStatus> {
        // Broadcasts arrive roughly once per second; wait for the next battery
        // monitor frame.
        match tokio::time::timeout(Duration::from_secs(10), next_monitor(&mut self.stream)).await {
            Ok(Some(s)) => Ok(s),
            Ok(None) => Err(Error::Transport("victron stream ended".into())),
            Err(_) => Err(Error::Timeout),
        }
    }

    fn has_stream(&self) -> bool {
        true
    }

    /// Victron broadcasts Instant Readout frames continuously, so this decodes
    /// the push stream directly instead of polling.
    fn stream(&mut self) -> Option<crate::battery::StatusStream<'_>> {
        use std::collections::VecDeque;
        type State<'a> = (
            &'a mut VictronMonitor,
            Option<BatteryStatus>,
            VecDeque<crate::StatusUpdate>,
            bool,
        );
        let init: State = (self, None, VecDeque::new(), false);
        let stream = futures_util::stream::unfold(
            init,
            |(this, mut prev, mut queue, ended): State| async move {
                loop {
                    if let Some(u) = queue.pop_front() {
                        return Some((Ok(u), (this, prev, queue, ended)));
                    }
                    if ended {
                        return None;
                    }
                    match tokio::time::timeout(
                        Duration::from_secs(30),
                        next_monitor(&mut this.stream),
                    )
                    .await
                    {
                        Ok(Some(status)) => {
                            queue.extend(status.diff(prev.as_ref()));
                            prev = Some(status);
                        }
                        Ok(None) => {
                            return Some((
                                Err(Error::Transport("victron stream ended".into())),
                                (this, prev, queue, true),
                            ));
                        }
                        Err(_) => {
                            return Some((Err(Error::Timeout), (this, prev, queue, true)));
                        }
                    }
                }
            },
        );
        Some(Box::pin(stream))
    }
}
