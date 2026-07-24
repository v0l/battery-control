//! BLE transport for JBD modules: service `FF00`, notify `FF01`, write `FF02`.

use crate::error::{Error, Result};
use crate::transport::Transport;
use async_trait::async_trait;
use btleplug::api::{
    Central, Characteristic, Manager as _, Peripheral as _, ScanFilter, ValueNotification,
    WriteType,
};
use btleplug::platform::{Manager, Peripheral};
use futures_util::{Stream, StreamExt};
use std::pin::Pin;
use std::time::Duration;
use uuid::Uuid;

pub const SERVICE_UUID: Uuid = Uuid::from_u128(0x0000ff00_0000_1000_8000_00805f9b34fb);
pub const NOTIFY_UUID: Uuid = Uuid::from_u128(0x0000ff01_0000_1000_8000_00805f9b34fb);
pub const WRITE_UUID: Uuid = Uuid::from_u128(0x0000ff02_0000_1000_8000_00805f9b34fb);

/// A discovered BLE peripheral.
#[derive(Debug, Clone)]
pub struct BtDevice {
    pub id: String,
    pub name: Option<String>,
    pub rssi: Option<i16>,
}

async fn adapter() -> Result<btleplug::platform::Adapter> {
    let manager = Manager::new()
        .await
        .map_err(|e| Error::Transport(format!("bt manager: {e}")))?;
    manager
        .adapters()
        .await
        .map_err(|e| Error::Transport(format!("bt adapters: {e}")))?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Transport("no bluetooth adapter found".into()))
}

/// Scan for JBD modules, filtered by the advertised `FF00` service UUID.
pub async fn scan(secs: u64) -> Result<Vec<BtDevice>> {
    let adapter = adapter().await?;
    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(|e| Error::Transport(format!("bt scan: {e}")))?;
    tokio::time::sleep(Duration::from_secs(secs.max(1))).await;

    let peripherals = adapter
        .peripherals()
        .await
        .map_err(|e| Error::Transport(format!("bt peripherals: {e}")))?;

    let mut out = Vec::new();
    for p in peripherals {
        let (name, rssi, services) = match p.properties().await {
            Ok(Some(props)) => (props.local_name, props.rssi, props.services),
            _ => (None, None, Vec::new()),
        };
        // Primary signal: advertises the JBD service. Fallback: a name hint,
        // since some modules don't list services in the advertisement.
        let name_hint = name
            .as_deref()
            .map(|n| {
                let l = n.to_ascii_lowercase();
                l.contains("xiaoxiang") || l.contains("jbd") || l.starts_with("sp")
            })
            .unwrap_or(false);
        if services.contains(&SERVICE_UUID) || name_hint {
            out.push(BtDevice {
                id: p.id().to_string(),
                name,
                rssi,
            });
        }
    }
    Ok(out)
}

type NotificationStream = Pin<Box<dyn Stream<Item = ValueNotification> + Send>>;

pub struct BluetoothTransport {
    target: String,
    peripheral: Option<Peripheral>,
    notify: Option<Characteristic>,
    write: Option<Characteristic>,
    notifications: Option<NotificationStream>,
    leftover: Vec<u8>,
}

impl BluetoothTransport {
    /// `target` is the stable [`btleplug::api::Peripheral::id`] string.
    pub fn new(target: &str) -> Self {
        Self {
            target: target.to_string(),
            peripheral: None,
            notify: None,
            write: None,
            notifications: None,
            leftover: Vec::new(),
        }
    }
}

#[async_trait]
impl Transport for BluetoothTransport {
    async fn open(&mut self) -> Result<()> {
        let adapter = adapter().await?;
        adapter
            .start_scan(ScanFilter::default())
            .await
            .map_err(|e| Error::Transport(format!("bt scan: {e}")))?;
        tokio::time::sleep(Duration::from_secs(3)).await;

        let peripherals = adapter
            .peripherals()
            .await
            .map_err(|e| Error::Transport(format!("bt peripherals: {e}")))?;

        let target = &self.target;
        let peripheral = peripherals
            .into_iter()
            .find(|p| {
                let id = p.id().to_string();
                id == *target || id.eq_ignore_ascii_case(target)
            })
            .ok_or(Error::NotFound)?;

        peripheral
            .connect()
            .await
            .map_err(|e| Error::Transport(format!("bt connect: {e}")))?;
        peripheral
            .discover_services()
            .await
            .map_err(|e| Error::Transport(format!("bt discover: {e}")))?;

        let chars = peripheral.characteristics();
        let notify = chars
            .iter()
            .find(|c| c.uuid == NOTIFY_UUID)
            .cloned()
            .ok_or_else(|| Error::Transport("notify characteristic ff01 not found".into()))?;
        // Some modules expose a single ff01 for both; fall back to it for writes.
        let write = chars
            .iter()
            .find(|c| c.uuid == WRITE_UUID)
            .or_else(|| chars.iter().find(|c| c.uuid == NOTIFY_UUID))
            .cloned()
            .ok_or_else(|| Error::Transport("write characteristic ff02 not found".into()))?;

        peripheral
            .subscribe(&notify)
            .await
            .map_err(|e| Error::Transport(format!("bt subscribe: {e}")))?;
        let notifications = peripheral
            .notifications()
            .await
            .map_err(|e| Error::Transport(format!("bt notifications: {e}")))?;

        self.notifications = Some(notifications);
        self.notify = Some(notify);
        self.write = Some(write);
        self.peripheral = Some(peripheral);
        self.leftover.clear();
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        if let Some(p) = self.peripheral.take() {
            let _ = p.disconnect().await;
        }
        self.notifications = None;
        self.notify = None;
        self.write = None;
        Ok(())
    }

    async fn write(&mut self, data: &[u8]) -> Result<usize> {
        let p = self.peripheral.as_ref().ok_or(Error::NotFound)?;
        let c = self.write.as_ref().ok_or(Error::NotFound)?;
        self.leftover.clear();
        p.write(c, data, WriteType::WithoutResponse)
            .await
            .map_err(|e| Error::Transport(format!("bt write: {e}")))?;
        Ok(data.len())
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if !self.leftover.is_empty() {
            let len = self.leftover.len().min(buf.len());
            buf[..len].copy_from_slice(&self.leftover[..len]);
            self.leftover.drain(..len);
            return Ok(len);
        }

        let stream = self.notifications.as_mut().ok_or(Error::NotFound)?;
        let first = match tokio::time::timeout(Duration::from_secs(3), stream.next()).await {
            Ok(Some(n)) => n.value,
            _ => return Ok(0),
        };
        let mut assembled = first;
        // Coalesce back-to-back notifications so a whole frame arrives per read.
        while let Ok(Some(n)) =
            tokio::time::timeout(Duration::from_millis(50), stream.next()).await
        {
            assembled.extend_from_slice(&n.value);
        }

        let len = assembled.len().min(buf.len());
        buf[..len].copy_from_slice(&assembled[..len]);
        if len < assembled.len() {
            self.leftover = assembled.split_off(len);
        }
        Ok(len)
    }
}
