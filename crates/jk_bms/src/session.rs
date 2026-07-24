use crate::error::{JkError, Result};
use crate::pack::MybmmPack;
use crate::module::{MybmmModule, Transport};

pub struct JkSession {
    pub pp: MybmmPack,
    pub tp: MybmmModule,
    pub tp_handle: Option<Box<dyn Transport>>,
}

impl JkSession {
    pub fn new(pp: MybmmPack, tp: MybmmModule) -> Result<Self> {
        let handle = tp.new_transport(pp.target.as_str(), pp.opts.as_str())?;
        
        Ok(Self {
            pp,
            tp,
            tp_handle: Some(handle),
        })
    }

    pub async fn open(&mut self) -> Result<()> {
        if let Some(ref mut handle) = self.tp_handle {
            handle.open().await
        } else {
            Err(JkError::TransportNotInitialized)
        }
    }

    pub async fn close(&mut self) -> Result<()> {
        if let Some(ref mut handle) = self.tp_handle {
            handle.close().await
        } else {
            Err(JkError::TransportNotInitialized)
        }
    }

    pub fn control(&mut self, op: u32, action: u32) -> Result<()> {
        if op == 1 {
            log::debug!("Charge control: action = {}", action);
        } else if op == 2 {
            log::debug!("Discharge control: action = {}", action);
        } else if op == 4 {
            log::debug!("Balance control: action = {}", action);
        } else {
            log::debug!("Unknown control op: {}", op);
        }
        Ok(())
    }
}
