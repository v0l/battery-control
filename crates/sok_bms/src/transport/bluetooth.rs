//! BLE transport for SOK batteries. Two generations are supported and detected
//! by which service the device exposes:
//! - **EE** (`0xEE` commands): service `FFE0`, notify `FFE1`, write `FFE2`.
//! - **ABC** (Modbus): service `FFF0`, notify `FFF1`, write `FFF2`.

use crate::data::{Identity, Variant};
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

const fn uuid16(x: u16) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_1000_8000_0080_5f9b_34fb | ((x as u128) << 96))
}

const EE_SERVICE: Uuid = uuid16(0xffe0);
const EE_NOTIFY: Uuid = uuid16(0xffe1);
const EE_WRITE: Uuid = uuid16(0xffe2);
const ABC_SERVICE: Uuid = uuid16(0xfff0);
const ABC_NOTIFY: Uuid = uuid16(0xfff1);
const ABC_WRITE: Uuid = uuid16(0xfff2);

// Standard Device Information Service (0x180A) characteristics.
const DIS_MANUFACTURER: Uuid = uuid16(0x2a29);
const DIS_MODEL: Uuid = uuid16(0x2a24);
const DIS_SERIAL: Uuid = uuid16(0x2a25);
const DIS_FIRMWARE: Uuid = uuid16(0x2a26);
const DIS_HARDWARE: Uuid = uuid16(0x2a27);

/// Read the standard BLE Device Information Service strings, ignoring any that
/// are missing or unreadable.
async fn read_identity(p: &Peripheral, chars: &std::collections::BTreeSet<Characteristic>) -> Identity {
    async fn s(
        p: &Peripheral,
        chars: &std::collections::BTreeSet<Characteristic>,
        uuid: Uuid,
    ) -> Option<String> {
        let c = chars.iter().find(|c| c.uuid == uuid)?;
        let bytes = p.read(c).await.ok()?;
        let txt = String::from_utf8_lossy(&bytes)
            .trim_matches(|c: char| c == '\0' || c.is_whitespace())
            .to_string();
        (!txt.is_empty()).then_some(txt)
    }
    Identity {
        name: None,
        manufacturer: s(p, chars, DIS_MANUFACTURER).await,
        model: s(p, chars, DIS_MODEL).await,
        serial: s(p, chars, DIS_SERIAL).await,
        firmware: s(p, chars, DIS_FIRMWARE).await,
        hardware: s(p, chars, DIS_HARDWARE).await,
    }
}

/// A discovered BLE peripheral.
#[derive(Debug, Clone)]
pub struct BtDevice {
    pub id: String,
    pub name: Option<String>,
    pub rssi: Option<i16>,
    /// Variant inferred from the advertisement, if determinable.
    pub variant: Option<Variant>,
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

fn name_is_sok(name: Option<&str>) -> bool {
    name.map(|n| {
        let l = n.to_ascii_lowercase();
        l.starts_with("sok") || l.starts_with("sk") || l.starts_with("abc")
    })
    .unwrap_or(false)
}

/// Scan for SOK batteries of either generation. Matches on the advertised
/// service (`FFE0`/`FFF0`) or a `SOK`/`SK`/`ABC` name prefix — both SOK
/// generations share their service range with other vendors (e.g. JK on
/// `FFE0`), so the name prefix disambiguates.
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
        let adv_variant = if services.contains(&ABC_SERVICE) {
            Some(Variant::Abc)
        } else if services.contains(&EE_SERVICE) {
            Some(Variant::Ee)
        } else {
            None
        };
        // Require a SOK-ish name so we don't claim a JK pack on FFE0.
        if name_is_sok(name.as_deref()) {
            out.push(BtDevice {
                id: p.id().to_string(),
                name,
                rssi,
                variant: adv_variant,
            });
        }
    }
    Ok(out)
}

/// Connect and list every GATT service/characteristic (a diagnostic used to
/// tell what a device actually supports — advertised services can lie).
pub async fn inspect(id: &str) -> Result<String> {
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
    let p = peripherals
        .into_iter()
        .find(|p| p.id().to_string().eq_ignore_ascii_case(id))
        .ok_or(Error::NotFound)?;
    p.connect()
        .await
        .map_err(|e| Error::Transport(format!("bt connect: {e}")))?;
    p.discover_services()
        .await
        .map_err(|e| Error::Transport(format!("bt discover: {e}")))?;

    let mut chars: Vec<_> = p.characteristics().into_iter().collect();
    chars.sort_by_key(|c| (c.service_uuid, c.uuid));
    let mut out = String::new();
    let mut last_service = None;
    for c in chars {
        if last_service != Some(c.service_uuid) {
            out.push_str(&format!("service {}\n", c.service_uuid));
            last_service = Some(c.service_uuid);
        }
        out.push_str(&format!("  char {}  {:?}\n", c.uuid, c.properties));
    }
    let _ = p.disconnect().await;
    Ok(out)
}

type NotificationStream = Pin<Box<dyn Stream<Item = ValueNotification> + Send>>;

pub struct BluetoothTransport {
    target: String,
    peripheral: Option<Peripheral>,
    write: Option<Characteristic>,
    notifications: Option<NotificationStream>,
    variant: Variant,
    identity: Identity,
}

impl BluetoothTransport {
    pub fn new(target: &str) -> Self {
        Self {
            target: target.to_string(),
            peripheral: None,
            write: None,
            notifications: None,
            variant: Variant::Abc, // provisional; set for real in `open`
            identity: Identity::default(),
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
        let has = |u: Uuid| chars.iter().any(|c| c.uuid == u);

        // Prefer ABC (Modbus) when present, else fall back to the EE service.
        let (variant, notify_uuid, write_uuid) = if has(ABC_NOTIFY) && has(ABC_WRITE) {
            (Variant::Abc, ABC_NOTIFY, ABC_WRITE)
        } else if has(EE_NOTIFY) && has(EE_WRITE) {
            (Variant::Ee, EE_NOTIFY, EE_WRITE)
        } else {
            return Err(Error::Transport(
                "no SOK service (FFF0/FFE0) on device".into(),
            ));
        };

        let notify = chars.iter().find(|c| c.uuid == notify_uuid).cloned().unwrap();
        let write = chars.iter().find(|c| c.uuid == write_uuid).cloned().unwrap();

        let mut identity = read_identity(&peripheral, &chars).await;
        identity.name = peripheral
            .properties()
            .await
            .ok()
            .flatten()
            .and_then(|p| p.local_name);
        self.identity = identity;

        peripheral
            .subscribe(&notify)
            .await
            .map_err(|e| Error::Transport(format!("bt subscribe: {e}")))?;
        let notifications = peripheral
            .notifications()
            .await
            .map_err(|e| Error::Transport(format!("bt notifications: {e}")))?;

        self.notifications = Some(notifications);
        self.write = Some(write);
        self.peripheral = Some(peripheral);
        self.variant = variant;
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
        match tokio::time::timeout(Duration::from_secs(3), stream.next()).await {
            Ok(Some(n)) => Ok(n.value),
            _ => Ok(Vec::new()),
        }
    }

    fn variant(&self) -> Variant {
        self.variant
    }

    fn identity(&self) -> Identity {
        self.identity.clone()
    }
}
