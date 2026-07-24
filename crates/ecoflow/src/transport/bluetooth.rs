//! BLE transport for EcoFlow: write `0002`, notify `0003`. Devices advertise
//! under manufacturer id `0xB5B5`; the serial is carried in the manufacturer
//! data (bytes 1..17).

use crate::error::{Error, Result};
use crate::transport::{Transport, SUPPORTED_PREFIXES};
use async_trait::async_trait;
use ble_util::uuid16;
use btleplug::api::{
    Central, Characteristic, Manager as _, Peripheral as _, ScanFilter, ValueNotification,
    WriteType,
};
use btleplug::platform::{Manager, Peripheral};
use futures_util::{Stream, StreamExt};
use std::pin::Pin;
use std::time::Duration;
use uuid::Uuid;

const MANUFACTURER_KEY: u16 = 0xB5B5;
const WRITE_UUID: Uuid = uuid16(0x0002);
const NOTIFY_UUID: Uuid = uuid16(0x0003);

/// A discovered EcoFlow device.
#[derive(Debug, Clone)]
pub struct BtDevice {
    pub id: String,
    pub name: Option<String>,
    pub rssi: Option<i16>,
    pub serial: String,
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

fn serial_from(props: &btleplug::api::PeripheralProperties) -> Option<String> {
    let data = props.manufacturer_data.get(&MANUFACTURER_KEY)?;
    if data.len() < 17 {
        return None;
    }
    let sn = std::str::from_utf8(&data[1..17]).ok()?;
    if SUPPORTED_PREFIXES.iter().any(|p| sn.starts_with(p)) {
        Some(sn.to_string())
    } else {
        None
    }
}

/// Scan for supported EcoFlow devices (HD31 / Y711) over BLE.
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
        let props = match p.properties().await {
            Ok(Some(props)) => props,
            _ => continue,
        };
        if let Some(serial) = serial_from(&props) {
            out.push(BtDevice {
                id: p.id().to_string(),
                name: props.local_name,
                rssi: props.rssi,
                serial,
            });
        }
    }
    Ok(out)
}

type NotificationStream = Pin<Box<dyn Stream<Item = ValueNotification> + Send>>;

pub struct BluetoothTransport {
    target: String,
    peripheral: Option<Peripheral>,
    write: Option<Characteristic>,
    notifications: Option<NotificationStream>,
    identity: ble_util::Identity,
}

impl BluetoothTransport {
    pub fn new(target: &str) -> Self {
        Self {
            target: target.to_string(),
            peripheral: None,
            write: None,
            notifications: None,
            identity: ble_util::Identity::default(),
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
            .ok_or_else(|| Error::Transport("notify characteristic 0003 not found".into()))?;
        let write = chars
            .iter()
            .find(|c| c.uuid == WRITE_UUID)
            .cloned()
            .ok_or_else(|| Error::Transport("write characteristic 0002 not found".into()))?;

        peripheral
            .subscribe(&notify)
            .await
            .map_err(|e| Error::Transport(format!("bt subscribe: {e}")))?;
        let notifications = peripheral
            .notifications()
            .await
            .map_err(|e| Error::Transport(format!("bt notifications: {e}")))?;

        self.identity = ble_util::read_identity(&peripheral).await;
        self.notifications = Some(notifications);
        self.write = Some(write);
        self.peripheral = Some(peripheral);
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        if let Some(p) = self.peripheral.take() {
            let _ = p.disconnect().await;
        }
        self.notifications = None;
        self.write = None;
        Ok(())
    }

    async fn write(&mut self, data: &[u8]) -> Result<usize> {
        let p = self.peripheral.as_ref().ok_or(Error::NotFound)?;
        let c = self.write.as_ref().ok_or(Error::NotFound)?;
        p.write(c, data, WriteType::WithoutResponse)
            .await
            .map_err(|e| Error::Transport(format!("bt write: {e}")))?;
        Ok(data.len())
    }

    async fn read_frame(&mut self) -> Result<Vec<u8>> {
        let stream = self.notifications.as_mut().ok_or(Error::NotFound)?;
        match tokio::time::timeout(Duration::from_secs(8), stream.next()).await {
            Ok(Some(n)) => Ok(n.value),
            _ => Ok(Vec::new()),
        }
    }

    fn identity(&self) -> ble_util::Identity {
        self.identity.clone()
    }
}
