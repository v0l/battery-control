use jk_bms::{Transport, Result, JkError, async_trait};
use std::time::{Duration, Instant};
use std::os::unix::io::{AsRawFd, RawFd};
use tokio::io::unix::AsyncFd;

/// Minimal owned wrapper so a RawFd can be registered with tokio's reactor.
struct Fd(RawFd);
impl AsRawFd for Fd {
    fn as_raw_fd(&self) -> RawFd { self.0 }
}
impl Drop for Fd {
    fn drop(&mut self) {
        if self.0 >= 0 {
            unsafe { libc::close(self.0); }
        }
    }
}

/// CAN frame structure for Linux CAN sockets
#[repr(C)]
#[derive(Debug, Clone)]
struct CanFrame {
    can_id: u32,
    data: [u8; 8],
    len: u8,
}

const CAN_SFF_MASK: u32 = 0x1FFFFFF;

pub struct CanTransport {
    interface: String,
    rx_id: u32,
    tx_id: u32,
    afd: Option<AsyncFd<Fd>>,
}

impl CanTransport {
    pub fn new(interface: &str, rx_id: u32, tx_id: u32) -> Self {
        Self {
            interface: interface.to_string(),
            rx_id,
            tx_id,
            afd: None,
        }
    }

    pub fn from_target(target: &str) -> Result<Self> {
        // Parse format: can:can0,0x18ff0000,0x18fe0000
        let parts: Vec<&str> = target.split(',').collect();
        if parts.len() < 3 {
            return Err(JkError::TransportError(
                "Invalid CAN target format. Use: can:can0,rx_id,tx_id".to_string()
            ));
        }

        let interface = parts[0].trim_start_matches("can:");
        let rx_id = u32::from_str_radix(parts[1].trim_start_matches("0x"), 16)
            .map_err(|_| JkError::TransportError("Invalid RX CAN ID".to_string()))?;
        let tx_id = u32::from_str_radix(parts[2].trim_start_matches("0x"), 16)
            .map_err(|_| JkError::TransportError("Invalid TX CAN ID".to_string()))?;

        Ok(Self::new(interface, rx_id, tx_id))
    }

    fn open_socket(&self) -> Result<RawFd> {
        // Create CAN socket
        let fd = unsafe {
            libc::socket(libc::AF_CAN, libc::SOCK_RAW, libc::CAN_RAW)
        };

        if fd < 0 {
            return Err(JkError::TransportError(format!(
                "Failed to create CAN socket: errno {}", errno()
            )));
        }

        // Get interface index using ioctl
        let if_index = unsafe {
            let mut ifreq: libc::ifreq = std::mem::zeroed();
            let name_bytes = self.interface.as_bytes();
            // Copy bytes - type varies by architecture
            for (i, &b) in name_bytes.iter().enumerate() {
                if i < libc::IFNAMSIZ {
                    ifreq.ifr_name[i] = b as _;
                }
            }
            
            let test_fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
            if test_fd < 0 {
                return Err(JkError::TransportError("Failed to create socket for interface lookup".to_string()));
            }
            
            let ret = libc::ioctl(test_fd, libc::SIOCGIFINDEX, &mut ifreq);
            libc::close(test_fd);
            
            if ret < 0 {
                return Err(JkError::TransportError(format!(
                    "Failed to get interface index for {}: errno {}",
                    self.interface, errno()
                )));
            }
            
            ifreq.ifr_ifru.ifru_ifindex
        };

        if if_index < 0 {
            unsafe { libc::close(fd); }
            return Err(JkError::TransportError(format!(
                "Failed to get interface index for {}: invalid index",
                self.interface
            )));
        }

        // Bind to CAN interface
        #[repr(C)]
        struct SockaddrCan {
            sa_family: u16,
            can_ifindex: i32,
            _pad: [u8; 8],
        }
        
        let sockaddr = SockaddrCan {
            sa_family: libc::AF_CAN as u16,
            can_ifindex: if_index,
            _pad: [0; 8],
        };

        let ret = unsafe {
            libc::bind(
                fd,
                &sockaddr as *const _ as *const libc::sockaddr,
                std::mem::size_of::<SockaddrCan>() as libc::socklen_t,
            )
        };

        if ret < 0 {
            unsafe { libc::close(fd); }
            return Err(JkError::TransportError(format!(
                "Failed to bind CAN socket to {}: errno {}",
                self.interface, errno()
            )));
        }

        // Set RX filter for our message ID
        let filter = libc::can_filter {
            can_id: self.rx_id & CAN_SFF_MASK,
            can_mask: CAN_SFF_MASK,
        };

        let ret = unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_CAN_BASE,
                libc::CAN_RAW_FILTER,
                &filter as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::can_filter>() as libc::socklen_t,
            )
        };

        if ret < 0 {
            unsafe { libc::close(fd); }
            return Err(JkError::TransportError(format!(
                "Failed to set CAN filter: errno {}", errno()
            )));
        }

        // Set non-blocking mode
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL, 0) };
        if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            unsafe { libc::close(fd); }
            return Err(JkError::TransportError(format!(
                "Failed to set non-blocking mode: errno {}", errno()
            )));
        }

        Ok(fd)
    }

    fn write_can(fd: RawFd, frame: &CanFrame) -> std::io::Result<usize> {
        let bytes_written = unsafe {
            libc::write(fd, frame as *const _ as *const libc::c_void, std::mem::size_of::<CanFrame>())
        };
        if bytes_written < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(bytes_written as usize)
        }
    }

    fn read_can(fd: RawFd, frame: &mut CanFrame) -> std::io::Result<usize> {
        let bytes_read = unsafe {
            libc::read(fd, frame as *mut _ as *mut libc::c_void, std::mem::size_of::<CanFrame>())
        };
        if bytes_read < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(bytes_read as usize)
        }
    }
}

fn errno() -> i32 {
    unsafe { *libc::__errno_location() }
}

#[async_trait]
impl Transport for CanTransport {
    async fn open(&mut self) -> Result<()> {
        let fd = self.open_socket()?;
        
        // Try to bring up the interface (may need root)
        use std::process::Command;
        let _ = Command::new("ip").args(["link", "set", &self.interface, "up"]).output();

        let afd = AsyncFd::new(Fd(fd))
            .map_err(|e| JkError::TransportError(format!("register CAN fd with reactor: {}", e)))?;
        self.afd = Some(afd);
        log::info!("CAN transport opened on {} with RX=0x{:07X}, TX=0x{:07X}", 
                   self.interface, self.rx_id, self.tx_id);
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        // Dropping the AsyncFd deregisters from the reactor and closes the fd.
        self.afd = None;
        Ok(())
    }

    async fn write(&mut self, data: &[u8]) -> Result<usize> {
        let afd = self.afd.as_ref().ok_or(JkError::TransportNotInitialized)?;

        if data.len() > 8 {
            log::warn!("CAN frame too large ({} bytes), truncating to 8 bytes", data.len());
        }

        let mut frame = CanFrame {
            can_id: self.tx_id & CAN_SFF_MASK,
            data: [0u8; 8],
            len: data.len().min(8) as u8,
        };
        frame.data[..frame.len as usize].copy_from_slice(&data[..frame.len as usize]);

        loop {
            let mut guard = afd.writable().await
                .map_err(|_| JkError::WriteFailed(-1))?;
            match guard.try_io(|inner| Self::write_can(inner.as_raw_fd(), &frame)) {
                Ok(res) => return res.map(|_| frame.len as usize).map_err(|_| JkError::WriteFailed(-1)),
                Err(_would_block) => continue,
            }
        }
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let afd = self.afd.as_ref().ok_or(JkError::TransportNotInitialized)?;

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut total = 0;

        while total < buf.len() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { break; }

            let mut guard = match tokio::time::timeout(remaining, afd.readable()).await {
                Ok(Ok(g)) => g,
                Ok(Err(_)) => return Err(JkError::ReadFailed(-1)),
                Err(_) => break, // timed out
            };

            let mut frame = CanFrame { can_id: 0, data: [0u8; 8], len: 0 };
            match guard.try_io(|inner| Self::read_can(inner.as_raw_fd(), &mut frame)) {
                Ok(Ok(_)) => {
                    if frame.len > 0 {
                        let copy_len = (frame.len as usize).min(buf.len() - total);
                        buf[total..total + copy_len].copy_from_slice(&frame.data[..copy_len]);
                        total += copy_len;
                        break;
                    }
                }
                Ok(Err(_)) => return Err(JkError::ReadFailed(-1)),
                Err(_would_block) => continue,
            }
        }

        if total > 0 {
            log::debug!("CAN read {} bytes", total);
        }
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_transport_parsing() {
        let transport = CanTransport::from_target("can:can0,0x18ff0000,0x18fe0000");
        assert!(transport.is_ok());
        let t = transport.unwrap();
        assert_eq!(t.interface, "can0");
        assert_eq!(t.rx_id, 0x18ff0000);
        assert_eq!(t.tx_id, 0x18fe0000);
    }

    #[test]
    fn test_can_transport_parsing_without_dev() {
        let transport = CanTransport::from_target("can:/dev/can0,0x18ff0000,0x18fe0000");
        assert!(transport.is_ok());
        let t = transport.unwrap();
        assert_eq!(t.interface, "/dev/can0");
    }

    #[test]
    fn test_can_transport_invalid_format() {
        let transport = CanTransport::from_target("can:/dev/can0");
        assert!(transport.is_err());
    }

    #[test]
    fn test_can_transport_invalid_ids() {
        let transport = CanTransport::from_target("can:/dev/can0,invalid,0x18fe0000");
        assert!(transport.is_err());
    }
}
