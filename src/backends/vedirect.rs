//! Adapter for [`vedirect`] — Victron devices speaking the **VE.Direct** text
//! protocol over a serial/USB link (BMV / SmartShunt battery monitors).
//!
//! VE.Direct devices continuously stream key/value frames at 19200 baud; this
//! backend parses the latest battery-monitor block into a [`BatteryStatus`].
//! Read-only.

use crate::battery::Battery;
use crate::types::{BatteryStatus, Reading};
use crate::{Capabilities, DeviceInfo, Error, Result};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use vedirect::{Bmv700, Events, Parser};

/// VE.Direct runs at a fixed 19200 baud.
const VEDIRECT_BAUD: u32 = 19200;

/// A `Clone`able snapshot of the fields we surface (the crate's `Bmv700` is
/// not `Clone`, and holding it would borrow the parser).
#[derive(Clone, Default)]
struct Snapshot {
    voltage: f32,
    power: i32,
    soc: Option<f32>,
    ttg: i32,
}

/// Parser listener that stashes the most recent decoded block.
struct Collector {
    out: Arc<Mutex<Option<Snapshot>>>,
}

impl Events<Bmv700> for Collector {
    fn on_complete_block(&mut self, b: Bmv700) {
        *self.out.lock().unwrap() = Some(Snapshot {
            voltage: b.voltage,
            power: b.power,
            soc: b.soc,
            ttg: b.ttg,
        });
    }
}

/// A Victron VE.Direct battery monitor exposed through [`Battery`].
pub struct VedirectMonitor {
    port: SerialStream,
    info: DeviceInfo,
}

impl VedirectMonitor {
    /// Open a VE.Direct serial port, e.g. `"/dev/ttyUSB0"`.
    pub fn open(path: &str) -> Result<Self> {
        let port = tokio_serial::new(path, VEDIRECT_BAUD)
            .open_native_async()
            .map_err(|e| Error::Transport(format!("serial open {path}: {e}")))?;
        Ok(Self {
            port,
            info: DeviceInfo {
                backend: "vedirect".into(),
                model: Some("Victron VE.Direct".into()),
                ..Default::default()
            },
        })
    }

    /// Read serial bytes until a complete battery-monitor block decodes.
    async fn read_block(&mut self, timeout: Duration) -> Result<Snapshot> {
        let shared: Arc<Mutex<Option<Snapshot>>> = Arc::new(Mutex::new(None));
        let mut collector = Collector { out: shared.clone() };
        let mut parser = Parser::new(&mut collector);
        let mut buf = [0u8; 512];
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(s) = shared.lock().unwrap().clone() {
                return Ok(s);
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            match tokio::time::timeout(Duration::from_secs(2), self.port.read(&mut buf)).await {
                Ok(Ok(0)) => return Err(Error::Transport("serial port closed".into())),
                Ok(Ok(n)) => {
                    let _ = parser.feed(&buf[..n]);
                }
                Ok(Err(e)) => return Err(Error::Transport(format!("serial read: {e}"))),
                Err(_) => {} // inter-frame gap; keep waiting until the deadline
            }
        }
    }
}

fn to_status(s: &Snapshot) -> BatteryStatus {
    // The BMV700 profile reports power, not current directly; derive it.
    let current = if s.voltage.abs() > f32::EPSILON {
        s.power as f32 / s.voltage
    } else {
        0.0
    };
    let mut st = BatteryStatus::default();
    st.set(Reading::Voltage, Some(s.voltage as f64))
        .set(Reading::Current, Some(current as f64))
        .set(Reading::PowerIn, (s.power > 0).then_some(s.power as f64))
        .set(Reading::PowerOut, (s.power < 0).then(|| (-s.power) as f64))
        .set(Reading::Soc, s.soc.map(|v| v as f64));
    // TTG is -1 when not discharging (infinite).
    if s.ttg >= 0 {
        st.set(Reading::TimeRemainingH, Some(s.ttg as f64 / 60.0));
    }
    st
}

#[async_trait]
impl Battery for VedirectMonitor {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::READ_BASIC
    }

    async fn status(&mut self) -> Result<BatteryStatus> {
        let snap = self.read_block(Duration::from_secs(5)).await?;
        Ok(to_status(&snap))
    }
}
