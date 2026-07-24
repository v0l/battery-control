//! Backend-agnostic discovery and universal identification.
//!
//! Every battery is identified by a **hardware id** — the BLE address/UUID or
//! the serial port id — never by name (names collide, e.g. two identical
//! stations). Users refer to a battery by this id (or an unambiguous prefix of
//! it); the backend is inferred, never specified.
//!
//! Discovery probes every enabled transport:
//! * **BLE** is scanned passively (Anker SOLIX).
//! * **Serial** ports are enumerated and actively probed with each wired
//!   backend's protocol to identify what's attached (JK, Daly).

use crate::battery::Battery;
use crate::{Error, Result};
use serde::Serialize;

/// Loose device class, for display and grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceClass {
    Bms,
    PowerStation,
    Monitor,
}

/// A battery found during discovery, keyed by its hardware id.
#[derive(Debug, Clone, Serialize)]
pub struct Discovered {
    /// Universal hardware id, e.g. `ble:<addr-or-uuid>` or `serial:/dev/ttyUSB0`.
    pub id: String,
    /// Friendly, non-unique label (model/advertised name).
    pub label: String,
    /// Backend that handles this device (informational only).
    pub backend: &'static str,
    pub class: DeviceClass,
    #[serde(skip)]
    locator: Locator,
}

/// How to (re)connect to a discovered device. Private: users never see or type this.
#[derive(Debug, Clone)]
enum Locator {
    #[cfg(feature = "anker")]
    Anker {
        /// The exact discovered peripheral — connecting to it works on macOS
        /// where the BLE MAC is unavailable.
        dev: anker_solix::Discovered,
    },
    #[cfg(feature = "jk-serial")]
    JkSerial { port: String, baud: u32 },
    #[cfg(feature = "jk-ble")]
    JkBle { id: String },
    #[cfg(feature = "jbd-serial")]
    JbdSerial { port: String, baud: u32 },
    #[cfg(feature = "jbd-ble")]
    JbdBle { id: String },
    #[cfg(feature = "sok")]
    SokBle { id: String },
    #[cfg(feature = "renogy")]
    RenogyBle { id: String },
    #[cfg(feature = "daly")]
    DalySerial { port: String },
    #[cfg(feature = "pace")]
    PaceSerial { port: String, baud: u32, address: u8 },
    #[cfg(feature = "vedirect")]
    Vedirect { port: String },
    #[cfg(test)]
    Test,
}

/// Options controlling discovery.
#[derive(Debug, Clone)]
pub struct DiscoverOptions {
    /// Seconds to scan BLE.
    pub ble_secs: u64,
    /// Whether to enumerate and probe serial ports.
    pub probe_serial: bool,
    /// Baud rates to try when probing serial ports.
    pub serial_bauds: Vec<u32>,
    /// Per-port probe timeout (seconds).
    pub probe_timeout_secs: u64,
}

impl Default for DiscoverOptions {
    fn default() -> Self {
        Self {
            ble_secs: 6,
            probe_serial: true,
            serial_bauds: vec![9600, 115200],
            probe_timeout_secs: 4,
        }
    }
}

impl Discovered {
    /// Connect to this device and return it behind the unified trait.
    pub async fn connect(&self, ble_secs: u64) -> Result<Box<dyn Battery>> {
        let _ = ble_secs;
        match &self.locator {
            #[cfg(feature = "anker")]
            Locator::Anker { dev } => {
                let device = dev
                    .clone()
                    .connect()
                    .await
                    .map_err(|e| Error::Transport(e.to_string()))?;
                Ok(Box::new(crate::backends::AnkerBattery::from_device(device)))
            }
            #[cfg(feature = "jk-serial")]
            Locator::JkSerial { port, baud } => {
                let b = crate::backends::JkBattery::open_serial(port, *baud).await?;
                Ok(Box::new(b))
            }
            #[cfg(feature = "jk-ble")]
            Locator::JkBle { id } => {
                let b = crate::backends::JkBattery::connect_bluetooth(id).await?;
                Ok(Box::new(b))
            }
            #[cfg(feature = "jbd-serial")]
            Locator::JbdSerial { port, baud } => {
                let b = crate::backends::JbdBattery::open_serial(port, *baud).await?;
                Ok(Box::new(b))
            }
            #[cfg(feature = "jbd-ble")]
            Locator::JbdBle { id } => {
                let b = crate::backends::JbdBattery::connect_bluetooth(id).await?;
                Ok(Box::new(b))
            }
            #[cfg(feature = "sok")]
            Locator::SokBle { id } => {
                let b = crate::backends::SokBattery::connect_bluetooth(id).await?;
                Ok(Box::new(b))
            }
            #[cfg(feature = "renogy")]
            Locator::RenogyBle { id } => {
                let b = crate::backends::RenogyBattery::connect_bluetooth(id).await?;
                Ok(Box::new(b))
            }
            #[cfg(feature = "daly")]
            Locator::DalySerial { port } => {
                let b = crate::backends::DalyBattery::open_serial(port)?;
                Ok(Box::new(b))
            }
            #[cfg(feature = "pace")]
            Locator::PaceSerial { port, baud, address } => {
                let b = crate::backends::PaceBattery::open_serial(port, *baud, *address).await?;
                Ok(Box::new(b))
            }
            #[cfg(feature = "vedirect")]
            Locator::Vedirect { port } => {
                let b = crate::backends::VedirectMonitor::open(port)?;
                Ok(Box::new(b))
            }
            #[cfg(test)]
            Locator::Test => Err(Error::Unsupported),
            #[allow(unreachable_patterns)]
            _ => Err(Error::Unsupported),
        }
    }
}

/// Discover batteries across all enabled transports.
///
/// All arms (Anker BLE scan, JK BLE scan, serial probing) run **concurrently**,
/// so total time is roughly `max(ble_secs, slowest serial probe)` rather than
/// their sum.
pub async fn discover(opts: &DiscoverOptions) -> Result<Vec<Discovered>> {
    #[allow(unused_mut)]
    let mut found: Vec<Discovered> = Vec::new();

    #[cfg(any(
        feature = "anker",
        feature = "jk-ble",
        feature = "jk-serial",
        feature = "jbd-ble",
        feature = "jbd-serial",
        feature = "sok",
        feature = "renogy",
        feature = "daly",
        feature = "pace",
        feature = "vedirect"
    ))]
    {
        // SOK is scanned before JK: both advertise service ffe0, so listing SOK
        // first lets it claim its packs (by name) before the id-dedup runs.
        let (anker, sok, renogy, jk, jbd, serial) = tokio::join!(
            scan_anker(opts),
            scan_sok_ble(opts),
            scan_renogy_ble(opts),
            scan_jk_ble(opts),
            scan_jbd_ble(opts),
            scan_serial(opts)
        );
        for d in anker
            .into_iter()
            .chain(sok)
            .chain(renogy)
            .chain(jk)
            .chain(jbd)
            .chain(serial)
        {
            if !found.iter().any(|x| x.id == d.id) {
                found.push(d);
            }
        }
    }
    #[cfg(not(any(
        feature = "anker",
        feature = "jk-ble",
        feature = "jk-serial",
        feature = "jbd-ble",
        feature = "jbd-serial",
        feature = "sok",
        feature = "renogy",
        feature = "daly",
        feature = "pace",
        feature = "vedirect"
    )))]
    let _ = opts;

    Ok(found)
}

async fn scan_anker(opts: &DiscoverOptions) -> Vec<Discovered> {
    #[cfg(feature = "anker")]
    {
        match anker_solix::scan(opts.ble_secs).await {
            Ok(devices) => {
                return devices
                    .into_iter()
                    .map(|d| Discovered {
                        id: format!("ble:{}", d.id),
                        label: d.name.clone(),
                        backend: "anker",
                        class: DeviceClass::PowerStation,
                        locator: Locator::Anker { dev: d },
                    })
                    .collect();
            }
            Err(e) => log::warn!("BLE scan failed: {e}"),
        }
    }
    let _ = opts;
    Vec::new()
}

/// JK BMSes that advertise over BLE. `JkBms::scan` filters on the JK serial
/// service UUID (0xFFE0) — device names are user-customisable via the app, so
/// a name-based filter would miss renamed units.
///
/// Each pack is its own device: multi-pack setups (name suffix `-00`, `-01`,
/// …) advertise and answer BLE independently, each reporting only its own
/// data, so every pack is listed as a separate connectable battery.
async fn scan_jk_ble(opts: &DiscoverOptions) -> Vec<Discovered> {
    #[cfg(feature = "jk-ble")]
    {
        match jk_bms::JkBms::scan(opts.ble_secs).await {
            Ok(devices) => {
                return devices
                    .into_iter()
                    .map(|d| Discovered {
                        id: format!("ble:{}", d.id),
                        label: d.name.unwrap_or_else(|| "JK BMS".to_string()),
                        backend: "jk",
                        class: DeviceClass::Bms,
                        locator: Locator::JkBle { id: d.id },
                    })
                    .collect();
            }
            Err(e) => log::warn!("JK BLE scan failed: {e}"),
        }
    }
    let _ = opts;
    Vec::new()
}

/// JBD / Xiaoxiang / Overkill BMSes advertising over BLE. `jbd_bms::scan`
/// filters on the JBD service UUID (0xFF00); names are user-customisable.
async fn scan_jbd_ble(opts: &DiscoverOptions) -> Vec<Discovered> {
    #[cfg(feature = "jbd-ble")]
    {
        match jbd_bms::scan(opts.ble_secs).await {
            Ok(devices) => {
                return devices
                    .into_iter()
                    .map(|d| Discovered {
                        id: format!("ble:{}", d.id),
                        label: d.name.unwrap_or_else(|| "JBD BMS".to_string()),
                        backend: "jbd",
                        class: DeviceClass::Bms,
                        locator: Locator::JbdBle { id: d.id },
                    })
                    .collect();
            }
            Err(e) => log::warn!("JBD BLE scan failed: {e}"),
        }
    }
    let _ = opts;
    Vec::new()
}

/// SOK batteries (both generations) advertising over BLE, filtered by a
/// SOK/SK/ABC name prefix (they share service ranges with other vendors).
async fn scan_sok_ble(opts: &DiscoverOptions) -> Vec<Discovered> {
    #[cfg(feature = "sok")]
    {
        match sok_bms::scan(opts.ble_secs).await {
            Ok(devices) => {
                return devices
                    .into_iter()
                    .map(|d| Discovered {
                        id: format!("ble:{}", d.id),
                        label: d.name.unwrap_or_else(|| "SOK".to_string()),
                        backend: "sok",
                        class: DeviceClass::Bms,
                        locator: Locator::SokBle { id: d.id },
                    })
                    .collect();
            }
            Err(e) => log::warn!("SOK BLE scan failed: {e}"),
        }
    }
    let _ = opts;
    Vec::new()
}

/// Renogy smart batteries advertising over BLE (BT-1/BT-2), filtered by the
/// `BT-TH`/`RBT` name prefix.
async fn scan_renogy_ble(opts: &DiscoverOptions) -> Vec<Discovered> {
    #[cfg(feature = "renogy")]
    {
        match renogy_bms::scan(opts.ble_secs).await {
            Ok(devices) => {
                return devices
                    .into_iter()
                    .map(|d| Discovered {
                        id: format!("ble:{}", d.id),
                        label: d.name.unwrap_or_else(|| "Renogy".to_string()),
                        backend: "renogy",
                        class: DeviceClass::Bms,
                        locator: Locator::RenogyBle { id: d.id },
                    })
                    .collect();
            }
            Err(e) => log::warn!("Renogy BLE scan failed: {e}"),
        }
    }
    let _ = opts;
    Vec::new()
}

async fn scan_serial(opts: &DiscoverOptions) -> Vec<Discovered> {
    #[cfg(any(
        feature = "jk-serial",
        feature = "jbd-serial",
        feature = "daly",
        feature = "pace",
        feature = "vedirect"
    ))]
    if opts.probe_serial {
        return probe_serial(opts).await;
    }
    let _ = opts;
    Vec::new()
}

/// Resolve a user query to exactly one discovered device.
///
/// Matching precedence: exact id, then unique id prefix, then unique
/// case-insensitive label substring. Ambiguous or missing matches error.
pub fn resolve<'a>(devices: &'a [Discovered], query: &str) -> Result<&'a Discovered> {
    if let Some(d) = devices.iter().find(|d| d.id == query) {
        return Ok(d);
    }
    let by_id: Vec<_> = devices.iter().filter(|d| d.id.starts_with(query)).collect();
    if by_id.len() == 1 {
        return Ok(by_id[0]);
    }
    if by_id.len() > 1 {
        return Err(Error::InvalidArgument(format!(
            "ambiguous id prefix '{query}' matches {} devices",
            by_id.len()
        )));
    }
    let q = query.to_ascii_lowercase();
    let by_label: Vec<_> = devices
        .iter()
        .filter(|d| d.label.to_ascii_lowercase().contains(&q))
        .collect();
    match by_label.len() {
        1 => Ok(by_label[0]),
        0 => Err(Error::NotFound(query.to_string())),
        n => Err(Error::InvalidArgument(format!(
            "'{query}' matches {n} devices by name; use a hardware id instead"
        ))),
    }
}

// --- Serial probing ----------------------------------------------------------

#[cfg(any(
    feature = "jk-serial",
    feature = "jbd-serial",
    feature = "daly",
    feature = "pace",
    feature = "vedirect"
))]
async fn probe_serial(opts: &DiscoverOptions) -> Vec<Discovered> {
    use std::time::Duration;

    let ports = match tokio_serial::available_ports() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("serial enumeration failed: {e}");
            return Vec::new();
        }
    };

    let timeout = Duration::from_secs(opts.probe_timeout_secs);
    let mut found = Vec::new();

    for info in ports {
        let port = info.port_name.clone();
        // Only probe likely USB/UART adapters; skip Bluetooth/modem tty aliases.
        if !looks_like_uart(&port) {
            continue;
        }
        if let Some(d) = probe_one_port(&port, opts, timeout).await {
            found.push(d);
        }
    }
    found
}

/// Only real USB/UART adapters are worth probing. Broad patterns like `cu.`
/// would match every macOS Bluetooth serial alias (headsets, phones, ...) and
/// `ttys` matches pseudo-terminals — probing those wastes ~10s each and can
/// never find a BMS.
#[cfg(any(
    feature = "jk-serial",
    feature = "jbd-serial",
    feature = "daly",
    feature = "pace",
    feature = "vedirect"
))]
fn looks_like_uart(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["ttyusb", "ttyacm", "usbserial", "usbmodem", "slab", "wchusbserial"]
        .iter()
        .any(|p| n.contains(p))
}

#[cfg(any(
    feature = "jk-serial",
    feature = "jbd-serial",
    feature = "daly",
    feature = "pace",
    feature = "vedirect"
))]
async fn probe_one_port(
    port: &str,
    opts: &DiscoverOptions,
    timeout: std::time::Duration,
) -> Option<Discovered> {
    // Try JK first (multiple bauds), then Daly. Each probe fully opens, reads a
    // status frame, and drops the handle before the next attempt.
    let _ = opts; // some feature combos (e.g. vedirect-only) use a fixed baud
    #[cfg(feature = "jk-serial")]
    for &baud in &opts.serial_bauds {
        let probe = async {
            let mut b = crate::backends::JkBattery::open_serial(port, baud).await.ok()?;
            b.status().await.ok().map(|_| b)
        };
        if let Ok(Some(b)) = tokio::time::timeout(timeout, probe).await {
            let label = b
                .info()
                .model
                .clone()
                .unwrap_or_else(|| "JK BMS".to_string());
            return Some(Discovered {
                id: format!("serial:{port}"),
                label,
                backend: "jk",
                class: DeviceClass::Bms,
                locator: Locator::JkSerial {
                    port: port.to_string(),
                    baud,
                },
            });
        }
    }

    #[cfg(feature = "jbd-serial")]
    for &baud in &opts.serial_bauds {
        let probe = async {
            let mut b = crate::backends::JbdBattery::open_serial(port, baud).await.ok()?;
            b.status().await.ok().map(|_| b)
        };
        if let Ok(Some(b)) = tokio::time::timeout(timeout, probe).await {
            let label = b
                .info()
                .model
                .clone()
                .unwrap_or_else(|| "JBD BMS".to_string());
            return Some(Discovered {
                id: format!("serial:{port}"),
                label,
                backend: "jbd",
                class: DeviceClass::Bms,
                locator: Locator::JbdSerial {
                    port: port.to_string(),
                    baud,
                },
            });
        }
    }

    #[cfg(feature = "daly")]
    {
        let probe = async {
            let mut b = crate::backends::DalyBattery::open_serial(port).ok()?;
            b.status().await.ok().map(|_| b)
        };
        if let Ok(Some(b)) = tokio::time::timeout(timeout, probe).await {
            let label = b
                .info()
                .model
                .clone()
                .unwrap_or_else(|| "Daly BMS".to_string());
            return Some(Discovered {
                id: format!("serial:{port}"),
                label,
                backend: "daly",
                class: DeviceClass::Bms,
                locator: Locator::DalySerial {
                    port: port.to_string(),
                },
            });
        }
    }

    // PACE-BMS rack packs: Modbus RTU at 9600, bus address 1 for a stand-alone
    // pack (multi-pack banks 1..N can be connected explicitly by address).
    #[cfg(feature = "pace")]
    {
        let probe = async {
            let mut b = crate::backends::PaceBattery::open_serial(port, 9600, 1).await.ok()?;
            b.status().await.ok().map(|_| b)
        };
        if let Ok(Some(b)) = tokio::time::timeout(timeout, probe).await {
            let label = b.info().model.clone().unwrap_or_else(|| "PACE BMS".to_string());
            return Some(Discovered {
                id: format!("serial:{port}"),
                label,
                backend: "pace",
                class: DeviceClass::Bms,
                locator: Locator::PaceSerial {
                    port: port.to_string(),
                    baud: 9600,
                    address: 1,
                },
            });
        }
    }

    // Victron VE.Direct streams text frames at a fixed 19200 baud.
    #[cfg(feature = "vedirect")]
    {
        let probe = async {
            let mut b = crate::backends::VedirectMonitor::open(port).ok()?;
            b.status().await.ok().map(|_| b)
        };
        if let Ok(Some(b)) = tokio::time::timeout(timeout, probe).await {
            let label = b
                .info()
                .model
                .clone()
                .unwrap_or_else(|| "Victron VE.Direct".to_string());
            return Some(Discovered {
                id: format!("serial:{port}"),
                label,
                backend: "vedirect",
                class: DeviceClass::Monitor,
                locator: Locator::Vedirect {
                    port: port.to_string(),
                },
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, label: &str) -> Discovered {
        Discovered {
            id: id.into(),
            label: label.into(),
            backend: "test",
            class: DeviceClass::Bms,
            locator: Locator::Test,
        }
    }

    #[test]
    fn resolve_prefers_exact_id() {
        let d = vec![dev("ble:AAAA", "C1000"), dev("ble:AAAB", "C1000")];
        assert_eq!(resolve(&d, "ble:AAAA").unwrap().id, "ble:AAAA");
    }

    #[test]
    fn resolve_unique_prefix() {
        let d = vec![dev("ble:AAAA", "C1000"), dev("serial:/dev/ttyUSB0", "JK")];
        assert_eq!(resolve(&d, "serial:").unwrap().backend, "test");
        assert_eq!(resolve(&d, "ble:AA").unwrap().id, "ble:AAAA");
    }

    #[test]
    fn resolve_ambiguous_prefix_errors() {
        let d = vec![dev("ble:AAAA", "C1000"), dev("ble:AAAB", "C1000 Gen 2")];
        assert!(resolve(&d, "ble:AAA").is_err());
    }

    #[test]
    fn resolve_by_label_when_unique() {
        let d = vec![dev("ble:AAAA", "SOLIX C1000"), dev("serial:/x", "JK PB2A16S")];
        assert_eq!(resolve(&d, "pb2").unwrap().id, "serial:/x");
    }

    #[test]
    fn resolve_duplicate_labels_error() {
        let d = vec![dev("ble:AAAA", "C1000"), dev("ble:AAAB", "C1000")];
        // name collision -> must use a hardware id
        assert!(resolve(&d, "c1000").is_err());
    }
}
