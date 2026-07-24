//! BLE transport for Jackery: service `BDEE`, write `EE01`, notify `EE02`.
//! `scan` also derives the encryption key from each advertisement.

use crate::error::{Error, Result};
use crate::key::{parse_advertisement, AdvInfo};
use crate::transport::Transport;
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

const SERVICE_DATA_UUID: Uuid = uuid16(0xbdee);
const WRITE_UUID: Uuid = uuid16(0xee01);
const NOTIFY_UUID: Uuid = uuid16(0xee02);

/// A discovered Jackery, with the key derived from its advertisement.
#[derive(Debug, Clone)]
pub struct BtDevice {
    pub id: String,
    pub name: Option<String>,
    pub rssi: Option<i16>,
    pub serial: String,
    pub model: u16,
    pub key: Vec<u8>,
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

fn derive(props: &btleplug::api::PeripheralProperties) -> Option<AdvInfo> {
    let service_data = props.service_data.get(&SERVICE_DATA_UUID)?;
    // A Jackery advertises exactly one manufacturer entry.
    let (&mfg_id, mfg_data) = props.manufacturer_data.iter().next()?;
    parse_advertisement(mfg_id, mfg_data, service_data)
}

/// Scan for Jackery devices, deriving each one's key from its advertisement.
/// Only devices whose advertisement decodes (CRC-valid) are returned.
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
        if let Some(info) = derive(&props) {
            let key = info.key();
            out.push(BtDevice {
                id: p.id().to_string(),
                name: props.local_name,
                rssi: props.rssi,
                serial: info.serial,
                model: info.model,
                key,
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
            .ok_or_else(|| Error::Transport("notify characteristic ee02 not found".into()))?;
        let write = chars
            .iter()
            .find(|c| c.uuid == WRITE_UUID)
            .cloned()
            .ok_or_else(|| Error::Transport("write characteristic ee01 not found".into()))?;

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
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(n)) => Ok(n.value),
            _ => Ok(Vec::new()),
        }
    }

    fn identity(&self) -> ble_util::Identity {
        self.identity.clone()
    }
}
