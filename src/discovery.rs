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
    #[cfg(feature = "jk")]
    JkSerial { port: String, baud: u32 },
    #[cfg(feature = "jk")]
    JkBle { id: String },
    #[cfg(feature = "daly")]
    DalySerial { port: String },
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
            #[cfg(feature = "jk")]
            Locator::JkSerial { port, baud } => {
                let b = crate::backends::JkBattery::open_serial(port, *baud).await?;
                Ok(Box::new(b))
            }
            #[cfg(feature = "jk")]
            Locator::JkBle { id } => {
                let b = crate::backends::JkBattery::connect_bluetooth(id).await?;
                Ok(Box::new(b))
            }
            #[cfg(feature = "daly")]
            Locator::DalySerial { port } => {
                let b = crate::backends::DalyBattery::open_serial(port)?;
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
pub async fn discover(opts: &DiscoverOptions) -> Result<Vec<Discovered>> {
    let mut found = Vec::new();

    #[cfg(feature = "anker")]
    {
        match anker_solix::scan(opts.ble_secs).await {
            Ok(devices) => {
                for d in devices {
                    found.push(Discovered {
                        id: format!("ble:{}", d.id),
                        label: d.name.clone(),
                        backend: "anker",
                        class: DeviceClass::PowerStation,
                        locator: Locator::Anker { dev: d },
                    });
                }
            }
            Err(e) => log::warn!("BLE scan failed: {e}"),
        }
    }

    // JK BMSes that advertise over BLE (identified by their "JK"-prefixed name).
    #[cfg(feature = "jk")]
    {
        match jk_bms::bt_scan().await {
            Ok(devices) => {
                for d in devices {
                    let name = d.name.unwrap_or_default();
                    if !name.to_ascii_uppercase().starts_with("JK") {
                        continue;
                    }
                    found.push(Discovered {
                        id: format!("ble:{}", d.id),
                        label: name,
                        backend: "jk",
                        class: DeviceClass::Bms,
                        locator: Locator::JkBle { id: d.id },
                    });
                }
            }
            Err(e) => log::warn!("JK BLE scan failed: {e}"),
        }
    }

    #[cfg(any(feature = "jk", feature = "daly"))]
    if opts.probe_serial {
        found.extend(probe_serial(opts).await);
    }

    Ok(found)
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

#[cfg(any(feature = "jk", feature = "daly"))]
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

#[cfg(any(feature = "jk", feature = "daly"))]
fn looks_like_uart(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["ttyusb", "ttyacm", "usbserial", "usbmodem", "cu.", "tty.slab", "ttys"]
        .iter()
        .any(|p| n.contains(p))
}

#[cfg(any(feature = "jk", feature = "daly"))]
async fn probe_one_port(
    port: &str,
    opts: &DiscoverOptions,
    timeout: std::time::Duration,
) -> Option<Discovered> {
    // Try JK first (multiple bauds), then Daly. Each probe fully opens, reads a
    // status frame, and drops the handle before the next attempt.
    #[cfg(feature = "jk")]
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
