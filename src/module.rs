use crate::error::{JkError, Result};
use crate::session::JkSession;
use crate::pack::MybmmPack;
use async_trait::async_trait;

#[async_trait]
pub trait Transport: Send {
    async fn open(&mut self) -> Result<()>;
    async fn close(&mut self) -> Result<()>;
    async fn write(&mut self, data: &[u8]) -> Result<usize>;
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
}

#[derive(Clone, Debug)]
pub struct MybmmModule {
    pub r#type: i32,
    pub name: &'static str,
    pub capabilities: u16,
}

impl MybmmModule {
    pub fn new(name: &'static str, capabilities: u16) -> Self {
        Self {
            r#type: MYBMM_MODTYPE_CELLMON,
            name,
            capabilities,
        }
    }

    pub fn with_type(r#type: i32, name: &'static str, capabilities: u16) -> Self {
        Self {
            r#type,
            name,
            capabilities,
        }
    }

    pub fn new_transport(&self, _target: &str, _opts: &str) -> Result<Box<dyn Transport>> {
        Err(JkError::TransportError("No transport implementation provided".to_string()))
    }
}

pub const MYBMM_MODTYPE_CELLMON: i32 = 1;

pub const MYBMM_CHARGE_CONTROL: u16 = 0x01;
pub const MYBMM_DISCHARGE_CONTROL: u16 = 0x02;
pub const MYBMM_BALANCE_CONTROL: u16 = 0x04;

pub fn jk_init(_conf: &mut dyn std::any::Any) -> Result<()> {
    Ok(())
}

pub fn jk_new(pp: MybmmPack, tp: MybmmModule) -> Result<JkSession> {
    JkSession::new(pp, tp)
}

pub async fn jk_open(session: &mut JkSession) -> Result<()> {
    session.open().await
}

pub async fn jk_read(session: &mut JkSession, pp: &mut MybmmPack) -> Result<()> {
    use crate::protocol::{get_info_command, get_cell_info_command, FrameAssembler};

    /// Reads attempted per command before giving up. Each transport read may
    /// itself block for a few seconds (BLE notification timeout).
    const READS_PER_COMMAND: u32 = 8;

    let handle = session
        .tp_handle
        .as_mut()
        .ok_or(JkError::TransportNotInitialized)?;

    let mut data = vec![0u8; 2048];
    // Frames are routinely fragmented across reads (BLE notifications) and
    // interleaved with short heartbeat packets, so every read is fed through
    // the assembler instead of being parsed raw — raw parsing silently drops
    // any frame that doesn't arrive whole and aligned in a single read.
    let mut asm = FrameAssembler::new();

    // Phase 1: device info (model, protocol version). Non-fatal if missed;
    // the pack keeps whatever was learned on a previous read.
    let info_cmd = get_info_command();
    let written = handle.write(&info_cmd).await?;
    log::debug!("Wrote {} bytes for getInfo", written);
    for _ in 0..READS_PER_COMMAND {
        let bytes = handle.read(&mut data).await?;
        log::debug!("Read {} bytes for getInfo", bytes);
        if bytes == 0 {
            continue;
        }
        if let Some(flags) = asm.feed_and_decode(pp, &data[..bytes]) {
            if flags.got_info {
                break;
            }
        }
    }

    // Phase 2: cell info (voltages, current, SOC, ...). Required.
    asm.clear();
    let cell_info_cmd = get_cell_info_command();
    let written = handle.write(&cell_info_cmd).await?;
    log::debug!("Wrote {} bytes for getCellInfo", written);
    for _ in 0..READS_PER_COMMAND {
        let bytes = handle.read(&mut data).await?;
        log::debug!("Read {} bytes for getCellInfo", bytes);
        if bytes == 0 {
            continue;
        }
        if let Some(flags) = asm.feed_and_decode(pp, &data[..bytes]) {
            if flags.got_volts {
                return Ok(());
            }
        }
    }

    Err(JkError::NoVoltageData)
}

pub async fn jk_close(session: &mut JkSession) -> Result<()> {
    session.close().await
}

pub fn jk_control(session: &mut JkSession, op: u32, action: u32) -> Result<()> {
    session.control(op, action)
}
