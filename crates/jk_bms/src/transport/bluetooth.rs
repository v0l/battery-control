use crate::{Transport, Result, JkError, async_trait};
use std::pin::Pin;
use std::time::Duration;

use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter, WriteType, Characteristic, ValueNotification};
use btleplug::platform::{Manager, Peripheral};
use futures_util::{Stream, StreamExt};
use uuid::Uuid;

/// A discovered BLE device.
#[derive(Debug, Clone)]
pub struct BtDevice {
    pub name: Option<String>,
    /// Stable, platform-independent identifier from btleplug's `Peripheral::id()`.
    /// Use this string as the transport target — unlike the MAC address it is not
    /// zeroed out on macOS.
    pub id: String,
    pub rssi: Option<i16>,
    /// Per-pack address parsed from the `-NN` name suffix JK-PB units
    /// advertise (`"REPT280-00"` → `0`). This is purely a user-assigned
    /// label/address: every pack advertises and answers BLE **independently**,
    /// each reporting only its own data — it is *not* a master/slave BLE
    /// hierarchy. `None` when there's no two-digit suffix.
    pub address: Option<u8>,
}

/// Parse the per-pack address from a device name's trailing `-NN` suffix
/// (exactly two ASCII digits), e.g. `"REPT280-00"` → `Some(0)`.
pub fn parse_pack_address(name: &str) -> Option<u8> {
    let (_, suffix) = name.rsplit_once('-')?;
    if suffix.len() == 2 && suffix.bytes().all(|b| b.is_ascii_digit()) {
        suffix.parse().ok()
    } else {
        None
    }
}

pub async fn scan() -> Result<Vec<BtDevice>> {
    let manager = Manager::new().await
        .map_err(|e| JkError::TransportError(format!("bt manager: {}", e)))?;

    let adapters = manager.adapters().await
        .map_err(|e| JkError::TransportError(format!("bt adapters: {}", e)))?;
    let adapter = adapters.into_iter().next()
        .ok_or_else(|| JkError::TransportError("no bluetooth adapter found".to_string()))?;

    adapter.start_scan(ScanFilter::default()).await
        .map_err(|e| JkError::TransportError(format!("bt scan: {}", e)))?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let peripherals = adapter.peripherals().await
        .map_err(|e| JkError::TransportError(format!("bt peripherals: {}", e)))?;

    let mut devices = Vec::new();
    for peripheral in peripherals {
        let mut name = None;
        let mut rssi = None;

        if let Ok(Some(props)) = peripheral.properties().await {
            name = props.local_name;
            rssi = props.rssi;
        }

        devices.push(BtDevice {
            address: name.as_deref().and_then(parse_pack_address),
            name,
            id: peripheral.id().to_string(),
            rssi,
        });
    }

    Ok(devices)
}

type NotificationStream = Pin<Box<dyn Stream<Item = ValueNotification> + Send>>;

pub struct BluetoothTransport {
    target: String,
    char_uuid_str: String,
    peripheral: Option<Peripheral>,
    characteristic: Option<Characteristic>,
    notifications: Option<NotificationStream>,
    leftover: Vec<u8>,
}

impl BluetoothTransport {
    pub fn new(target: &str, topts: Option<&str>) -> Self {
        let char_uuid = topts.map(|s| {
            let trimmed = s.trim();
            if let Some(hex) = trimmed.strip_prefix("0x") {
                if hex.len() <= 4 {
                    format!("0000{}-0000-1000-8000-00805f9b34fb", hex)
                } else {
                    trimmed.to_string()
                }
            } else if trimmed.len() == 36 && trimmed.contains('-') {
                trimmed.to_string()
            } else {
                // Short 16-bit UUID alias (with or without exactly 4 chars).
                format!("0000{}-0000-1000-8000-00805f9b34fb", trimmed)
            }
        }).unwrap_or_else(|| "0000ffe1-0000-1000-8000-00805f9b34fb".to_string());

        Self {
            target: target.to_string(),
            char_uuid_str: char_uuid,
            peripheral: None,
            characteristic: None,
            notifications: None,
            leftover: Vec::new(),
        }
    }

    pub fn from_target(target: &str) -> Self {
        let mut parts = target.split(',');
        let mac = parts.next().unwrap_or(target);
        let topts = parts.next();
        Self::new(mac, topts)
    }
}

#[async_trait]
impl Transport for BluetoothTransport {
    async fn open(&mut self) -> Result<()> {
        let target = self.target.clone();
        let char_uuid_str = self.char_uuid_str.clone();

        let manager = Manager::new().await
            .map_err(|e| JkError::TransportError(format!("bt manager: {}", e)))?;

        let adapters = manager.adapters().await
            .map_err(|e| JkError::TransportError(format!("bt adapters: {}", e)))?;
        let adapter = adapters.into_iter().next()
            .ok_or_else(|| JkError::TransportError("no bluetooth adapter found".to_string()))?;

        adapter.start_scan(ScanFilter::default()).await
            .map_err(|e| JkError::TransportError(format!("bt scan: {}", e)))?;
        tokio::time::sleep(Duration::from_secs(3)).await;

        let peripherals = adapter.peripherals().await
            .map_err(|e| JkError::TransportError(format!("bt peripherals: {}", e)))?;

        // Match on the stable `Peripheral::id()` string (works on macOS, where the
        // MAC address is zeroed). Also accept a case-insensitive match so a MAC-style
        // id can be given in either case.
        let peripheral = peripherals.into_iter()
            .find(|p| {
                let id = p.id().to_string();
                id == target || id.eq_ignore_ascii_case(&target)
            })
            .ok_or_else(|| JkError::TransportError(format!("device {} not found", target)))?;

        peripheral.connect().await
            .map_err(|e| JkError::TransportError(format!("bt connect: {}", e)))?;

        peripheral.discover_services().await
            .map_err(|e| JkError::TransportError(format!("bt discover: {}", e)))?;

        let characteristics = peripheral.characteristics();
        let char_uuid = Uuid::parse_str(&char_uuid_str)
            .map_err(|e| JkError::TransportError(format!("invalid uuid: {}", e)))?;

        let characteristic = characteristics.into_iter()
            .find(|c| c.uuid == char_uuid)
            .ok_or_else(|| JkError::TransportError(format!("characteristic {} not found", char_uuid_str)))?;

        peripheral.subscribe(&characteristic).await
            .map_err(|e| JkError::TransportError(format!("bt subscribe: {}", e)))?;

        let notifications = peripheral.notifications().await
            .map_err(|e| JkError::TransportError(format!("bt notifications: {}", e)))?;

        self.peripheral = Some(peripheral);
        self.characteristic = Some(characteristic);
        self.notifications = Some(notifications);
        self.leftover.clear();
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        self.notifications = None;
        if let Some(peripheral) = self.peripheral.take() {
            let _ = peripheral.disconnect().await;
        }
        self.characteristic = None;
        Ok(())
    }

    async fn write(&mut self, data: &[u8]) -> Result<usize> {
        let peripheral = self.peripheral.as_ref().ok_or(JkError::TransportNotInitialized)?;
        let characteristic = self.characteristic.as_ref().ok_or(JkError::TransportNotInitialized)?;

        // Drop any stale buffered notifications before issuing a new request.
        self.leftover.clear();

        peripheral.write(characteristic, data, WriteType::WithoutResponse)
            .await
            .map_err(|_e| JkError::WriteFailed(0))?;

        Ok(data.len())
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        // Serve any leftover bytes from a previous notification first.
        if !self.leftover.is_empty() {
            let len = std::cmp::min(self.leftover.len(), buf.len());
            buf[..len].copy_from_slice(&self.leftover[..len]);
            self.leftover.drain(..len);
            return Ok(len);
        }

        let stream = self.notifications.as_mut().ok_or(JkError::TransportNotInitialized)?;

        // Await the first notification (BLE-driven, no polling).
        let first = match tokio::time::timeout(Duration::from_secs(3), stream.next()).await {
            Ok(Some(n)) => n.value,
            _ => return Ok(0),
        };
        let mut assembled = first;

        // Drain any further notifications that arrive back-to-back so a full
        // BMS frame is returned in a single read, without a fixed sleep.
        while let Ok(Some(n)) = tokio::time::timeout(Duration::from_millis(50), stream.next()).await {
            assembled.extend_from_slice(&n.value);
        }

        let len = std::cmp::min(assembled.len(), buf.len());
        buf[..len].copy_from_slice(&assembled[..len]);
        if len < assembled.len() {
            self.leftover = assembled.split_off(len);
        }
        Ok(len)
    }
}
