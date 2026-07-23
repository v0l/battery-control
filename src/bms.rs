//! High-level device API: [`JkBms`].
//!
//! Mirrors the `scan()` → `connect()` → `read()` shape of sibling device
//! crates: one struct owns the session and pack, and every transport
//! (BLE / serial / CAN) drives the same assembler-based read path that the
//! `jktool` CLI uses.
//!
//! ```no_run
//! # async fn demo() -> jk_bms::Result<()> {
//! use jk_bms::JkBms;
//!
//! for dev in JkBms::scan(5).await? {
//!     println!("{} [{}]", dev.name.as_deref().unwrap_or("?"), dev.id);
//! }
//!
//! let mut bms = JkBms::connect_ble("aa:bb:cc:dd:ee:ff").await?;
//! let pack = bms.read().await?;
//! println!("SOC {:.0}%  {:.2} V", pack.soc, pack.voltage);
//! bms.disconnect().await?;
//! # Ok(()) }
//! ```

use crate::error::{JkError, Result};
use crate::module::{
    MybmmModule, Transport, MYBMM_BALANCE_CONTROL, MYBMM_CHARGE_CONTROL, MYBMM_DISCHARGE_CONTROL,
};
use crate::pack::{JkSettings, MybmmPack};
use crate::protocol::{
    build_can_setting_write_frame, build_setting_write_frame, get_can_cell_info_command,
    get_can_info_command, get_cell_info_command, get_info_command, FrameAssembler,
};
use crate::session::JkSession;

/// Read live data from the BMS into `pack`.
///
/// This is the one true read path (used by [`JkBms`], [`crate::jk_read`] and
/// the `jktool` CLI): commands are transport-aware (CAN vs serial/BLE) and
/// every read is fed through a [`FrameAssembler`], so frames fragmented
/// across BLE notifications or interleaved with heartbeats still decode.
///
/// Phase 1 requests device info (model, protocol version; non-fatal if
/// missed), phase 2 requests cell info and errors with
/// [`JkError::NoVoltageData`] if no voltage frame arrives. When
/// `need_settings` is true, phase 2 also waits for a settings frame.
pub async fn read_data(
    session: &mut JkSession,
    pack: &mut MybmmPack,
    need_settings: bool,
) -> Result<()> {
    const RETRIES: u32 = 5;
    const READ_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);
    const POLL: std::time::Duration = std::time::Duration::from_millis(50);

    let is_can = pack.transport == "can";
    let mut data = vec![0u8; 2048];
    let mut asm = FrameAssembler::new();

    // Phase 1: getInfo (model, protocol version). Non-fatal if missed; the
    // pack keeps whatever was learned on a previous read.
    let info_cmd: Vec<u8> = if is_can {
        get_can_info_command().to_vec()
    } else {
        get_info_command().to_vec()
    };
    let mut got_info = false;
    'info: for _ in 0..RETRIES {
        let handle = session
            .tp_handle
            .as_mut()
            .ok_or(JkError::TransportNotInitialized)?;
        handle.write(&info_cmd).await?;
        let start = std::time::Instant::now();
        while start.elapsed() < READ_WINDOW {
            let bytes = handle.read(&mut data).await?;
            log::debug!("read {} bytes for getInfo", bytes);
            if bytes > 0 {
                if let Some(flags) = asm.feed_and_decode(pack, &data[..bytes]) {
                    if flags.got_info {
                        got_info = true;
                        break 'info;
                    }
                }
            }
            tokio::time::sleep(POLL).await;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    if !got_info {
        log::warn!("no device info frame received; using last known protocol version");
    }

    // Phase 2: getCellInfo (voltages, current, SOC, and optionally settings).
    let cell_cmd: Vec<u8> = if is_can {
        get_can_cell_info_command().to_vec()
    } else {
        get_cell_info_command().to_vec()
    };
    let mut got_volts = false;
    let mut got_settings = pack.settings.is_some();
    asm.clear();
    'cell: for _ in 0..RETRIES {
        let handle = session
            .tp_handle
            .as_mut()
            .ok_or(JkError::TransportNotInitialized)?;
        handle.write(&cell_cmd).await?;
        let start = std::time::Instant::now();
        while start.elapsed() < READ_WINDOW {
            let bytes = handle.read(&mut data).await?;
            log::debug!("read {} bytes for getCellInfo", bytes);
            if bytes > 0 {
                if let Some(flags) = asm.feed_and_decode(pack, &data[..bytes]) {
                    got_volts |= flags.got_volts;
                    got_settings |= flags.got_res;
                    if got_volts && (!need_settings || got_settings) {
                        break 'cell;
                    }
                }
            }
            tokio::time::sleep(POLL).await;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    if !got_volts {
        return Err(JkError::NoVoltageData);
    }
    Ok(())
}

/// A connected JK BMS.
pub struct JkBms {
    session: JkSession,
    pack: MybmmPack,
    /// Assembler for the push stream ([`Self::next_update`]); persistent so
    /// partial frames survive across calls.
    stream_asm: FrameAssembler,
    /// Whether the cell-info request that starts the push stream was sent.
    streaming: bool,
}

impl JkBms {
    /// Scan BLE for JK BMS devices for `secs` seconds.
    ///
    /// Unlike [`crate::bt_scan`], this returns only peripherals that look
    /// like JK BMS units: they advertise the JK serial service (`0xFFE0`) or
    /// carry the default `JK` name prefix. Names are user-customisable via
    /// the app, so the service UUID is the reliable signal.
    #[cfg(feature = "bluetooth")]
    pub async fn scan(secs: u64) -> Result<Vec<crate::transport::BtDevice>> {
        use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
        use btleplug::platform::Manager;

        /// 0000ffe0-0000-1000-8000-00805f9b34fb — the UART-style service JK
        /// BMS units advertise.
        const JK_SERVICE: uuid::Uuid =
            uuid::Uuid::from_u128(0x0000ffe0_0000_1000_8000_00805f9b34fb);

        let err = |e: btleplug::Error| JkError::TransportError(e.to_string());
        let manager = Manager::new().await.map_err(err)?;
        let adapter = manager
            .adapters()
            .await
            .map_err(err)?
            .into_iter()
            .next()
            .ok_or_else(|| JkError::TransportError("no bluetooth adapter found".to_string()))?;

        adapter.start_scan(ScanFilter::default()).await.map_err(err)?;
        tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
        let _ = adapter.stop_scan().await;

        let mut out = Vec::new();
        for p in adapter.peripherals().await.map_err(err)? {
            let Ok(Some(props)) = p.properties().await else {
                continue;
            };
            let name = props.local_name.unwrap_or_default();
            let is_jk = props.services.contains(&JK_SERVICE)
                || name.to_ascii_uppercase().starts_with("JK");
            if !is_jk {
                continue;
            }
            out.push(crate::transport::BtDevice {
                name: (!name.is_empty()).then_some(name),
                id: p.id().to_string(),
                rssi: props.rssi,
            });
        }
        Ok(out)
    }

    /// Connect over BLE. `target` is the peripheral id from [`Self::scan`]
    /// (or a MAC address on platforms that expose it), optionally followed by
    /// `,<characteristic>` to override the default `ffe1`.
    #[cfg(feature = "bluetooth")]
    pub async fn connect_ble(target: &str) -> Result<Self> {
        let t = crate::transport::BluetoothTransport::from_target(target);
        Self::open("bt", target, Box::new(t)).await
    }

    /// Connect over serial. `target` is `<path>[,<baud>]`, e.g.
    /// `/dev/ttyUSB0,115200`.
    #[cfg(feature = "serial")]
    pub async fn connect_serial(target: &str) -> Result<Self> {
        let t = crate::transport::SerialTransport::from_target(target);
        Self::open("serial", target, Box::new(t)).await
    }

    /// Connect over CAN. `target` is `<iface>[,<rx-id>,<tx-id>]`, e.g.
    /// `can0,0x18ff0000,0x18fe0000`.
    #[cfg(all(feature = "can", target_os = "linux"))]
    pub async fn connect_can(target: &str) -> Result<Self> {
        let t = crate::transport::CanTransport::from_target(target)?;
        Self::open("can", target, Box::new(t)).await
    }

    /// Connect over a caller-supplied [`Transport`] (e.g. a mock in tests).
    /// `transport_name` selects the command dialect: `"can"` for CAN,
    /// anything else for the standard serial/BLE frames.
    pub async fn open(
        transport_name: &str,
        target: &str,
        transport: Box<dyn Transport>,
    ) -> Result<Self> {
        let mut pack = MybmmPack::new("jk");
        pack.transport = transport_name.to_string();
        pack.target = target.to_string();
        let module = MybmmModule::new(
            "jk",
            MYBMM_CHARGE_CONTROL | MYBMM_DISCHARGE_CONTROL | MYBMM_BALANCE_CONTROL,
        );
        let mut session = JkSession {
            pp: pack.clone(),
            tp: module,
            tp_handle: Some(transport),
        };
        session.open().await?;
        Ok(Self {
            session,
            pack,
            stream_asm: FrameAssembler::new(),
            streaming: false,
        })
    }

    /// Wait for the next pushed update and return the refreshed pack state.
    ///
    /// A JK BMS keeps broadcasting cell-info frames (roughly every second
    /// over BLE) after a single `getCellInfo` request — no polling needed.
    /// The first call sends that request; subsequent calls just decode the
    /// push stream. Errors with [`JkError::Timeout`] if no complete frame
    /// arrives within `timeout`.
    ///
    /// ```no_run
    /// # async fn demo(bms: &mut jk_bms::JkBms) -> jk_bms::Result<()> {
    /// loop {
    ///     let pack = bms.next_update(std::time::Duration::from_secs(10)).await?;
    ///     println!("SOC {:.0}%  {:.3} V", pack.soc, pack.voltage);
    /// }
    /// # }
    /// ```
    pub async fn next_update(&mut self, timeout: std::time::Duration) -> Result<&MybmmPack> {
        let is_can = self.pack.transport == "can";
        let handle = self
            .session
            .tp_handle
            .as_mut()
            .ok_or(JkError::TransportNotInitialized)?;

        if !self.streaming {
            let cell_cmd: Vec<u8> = if is_can {
                get_can_cell_info_command().to_vec()
            } else {
                get_cell_info_command().to_vec()
            };
            handle.write(&cell_cmd).await?;
            self.streaming = true;
        }

        let deadline = std::time::Instant::now() + timeout;
        let mut buf = vec![0u8; 2048];
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(JkError::Timeout);
            }
            let bytes = match tokio::time::timeout(remaining, handle.read(&mut buf)).await {
                Ok(res) => res?,
                Err(_) => return Err(JkError::Timeout),
            };
            if bytes == 0 {
                continue;
            }
            if let Some(flags) = self.stream_asm.feed_and_decode(&mut self.pack, &buf[..bytes]) {
                if flags.got_volts {
                    return Ok(&self.pack);
                }
            }
        }
    }

    /// Read live data (cell voltages, current, SOC, temperatures, ...) and
    /// return the full decoded pack state.
    pub async fn read(&mut self) -> Result<&MybmmPack> {
        read_data(&mut self.session, &mut self.pack, false).await?;
        Ok(&self.pack)
    }

    /// Read live data *and* wait for the settings frame.
    /// Errors if the BMS never sends one.
    pub async fn read_settings(&mut self) -> Result<JkSettings> {
        read_data(&mut self.session, &mut self.pack, true).await?;
        self.pack
            .settings
            .clone()
            .ok_or_else(|| JkError::TransportError("no settings frame received".to_string()))
    }

    /// Write a named setting (see [`crate::SETTINGS`] /
    /// `jktool list-settings`), e.g. `set("charge", "off")`.
    ///
    /// Reads first if the device has never been read, since write frames are
    /// protocol-specific and the protocol version comes from the info frame.
    pub async fn set(&mut self, name: &str, value: &str) -> Result<()> {
        if self.pack.model.is_empty() {
            let _ = self.read().await;
        }
        let frame: Vec<u8> = if self.pack.transport == "can" {
            build_can_setting_write_frame(name, value, self.pack.protocol_version)
                .map(|f| f.to_vec())
        } else {
            build_setting_write_frame(name, value, self.pack.protocol_version)
                .map(|f| f.to_vec())
        }
        .ok_or_else(|| {
            JkError::TransportError(format!(
                "unknown or unsupported setting '{}' for protocol {:?}",
                name, self.pack.protocol_version
            ))
        })?;

        let handle = self
            .session
            .tp_handle
            .as_mut()
            .ok_or(JkError::TransportNotInitialized)?;
        handle.write(&frame).await?;
        Ok(())
    }

    /// The raw decoded pack state from the last read.
    pub fn pack(&self) -> &MybmmPack {
        &self.pack
    }

    /// Model string reported by the BMS (empty until the first read).
    pub fn model(&self) -> &str {
        &self.pack.model
    }

    pub async fn disconnect(&mut self) -> Result<()> {
        self.session.close().await
    }
}
