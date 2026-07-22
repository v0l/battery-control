//! Adapter for [`victron_ble`] — Victron devices broadcasting "Instant Readout"
//! data over BLE. Read-only battery monitors (BMV/SmartShunt).
//!
//! Requires the device's name and its BLE encryption key (VictronConnect →
//! Settings → Product info → Encryption data).

use crate::battery::Battery;
use crate::types::BatteryStatus;
use crate::{Capabilities, DeviceInfo, Error, Result};
use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use std::pin::Pin;
use std::time::Duration;
use victron_ble::{BatteryMonitorState, DeviceState};

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
    BatteryStatus {
        soc: m.state_of_charge_pct,
        voltage: m.battery_voltage_v,
        current: m.battery_current_a,
        power_in: power.filter(|p| *p > 0.0),
        power_out: power.filter(|p| *p < 0.0).map(f32::abs),
        time_remaining_h: m.time_to_go_mins.map(|min| min / 60.0),
        ..Default::default()
    }
}

#[async_trait]
impl Battery for VictronMonitor {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::READ_BASIC
    }

    async fn status(&mut self) -> Result<BatteryStatus> {
        // Broadcasts arrive roughly once per second; wait for the next battery
        // monitor frame.
        let fut = async {
            while let Some(item) = self.stream.next().await {
                if let Ok(DeviceState::BatteryMonitor(m)) = item {
                    return Some(to_status(&m));
                }
            }
            None
        };
        match tokio::time::timeout(Duration::from_secs(10), fut).await {
            Ok(Some(s)) => Ok(s),
            Ok(None) => Err(Error::Transport("victron stream ended".into())),
            Err(_) => Err(Error::Timeout),
        }
    }
}
