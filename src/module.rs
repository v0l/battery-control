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

/// Read live data into `pp`. Thin wrapper over [`crate::bms::read_data`]
/// (the shared assembler-based read path); prefer [`crate::JkBms`] for new
/// code.
pub async fn jk_read(session: &mut JkSession, pp: &mut MybmmPack) -> Result<()> {
    crate::bms::read_data(session, pp, false).await
}

pub async fn jk_close(session: &mut JkSession) -> Result<()> {
    session.close().await
}

pub fn jk_control(session: &mut JkSession, op: u32, action: u32) -> Result<()> {
    session.control(op, action)
}
