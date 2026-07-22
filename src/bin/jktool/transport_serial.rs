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

pub struct SerialTransport {
    port_name: String,
    baud_rate: u32,
    afd: Option<AsyncFd<Fd>>,
}

impl SerialTransport {
    pub fn new(port_name: &str, baud_rate: u32) -> Self {
        Self {
            port_name: port_name.to_string(),
            baud_rate,
            afd: None,
        }
    }

    pub fn from_target(target: &str) -> Self {
        let mut parts = target.split(',');
        let port = parts.next().unwrap_or("/dev/ttyUSB0");
        let baud = parts.next().and_then(|s| s.parse().ok()).unwrap_or(9600);
        Self::new(port, baud)
    }
}

fn errno() -> i32 {
    unsafe { *libc::__errno_location() }
}

#[async_trait]
impl Transport for SerialTransport {
    async fn open(&mut self) -> Result<()> {
        use std::ffi::CString;
        let c_path = CString::new(self.port_name.as_bytes())
            .map_err(|_| JkError::TransportError("invalid path".to_string()))?;

        let fd = unsafe {
            libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK)
        };
        if fd < 0 {
            return Err(JkError::TransportError(format!(
                "open {}: errno {}", self.port_name, errno()
            )));
        }

        unsafe {
            let mut tty: libc::termios = std::mem::zeroed();
            libc::tcgetattr(fd, &mut tty);
            libc::cfmakeraw(&mut tty);
            tty.c_cc[libc::VMIN] = 0;
            tty.c_cc[libc::VTIME] = 50; // 5 seconds

            let rate = match self.baud_rate {
                9600 => libc::B9600,
                19200 => libc::B19200,
                38400 => libc::B38400,
                57600 => libc::B57600,
                115200 => libc::B115200,
                230400 => libc::B230400,
                _ => libc::B9600,
            };
            libc::cfsetispeed(&mut tty, rate);
            libc::cfsetospeed(&mut tty, rate);
            libc::tcsetattr(fd, libc::TCSANOW, &tty);
            libc::tcflush(fd, libc::TCIOFLUSH);
        }

        let afd = AsyncFd::new(Fd(fd))
            .map_err(|e| JkError::TransportError(format!("register fd with reactor: {}", e)))?;
        self.afd = Some(afd);
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        // Dropping the AsyncFd deregisters from the reactor and closes the fd.
        self.afd = None;
        Ok(())
    }

    async fn write(&mut self, data: &[u8]) -> Result<usize> {
        let afd = self.afd.as_ref().ok_or(JkError::TransportNotInitialized)?;
        loop {
            let mut guard = afd.writable().await
                .map_err(|_| JkError::WriteFailed(-1))?;
            match guard.try_io(|inner| {
                let n = unsafe {
                    libc::write(inner.as_raw_fd(), data.as_ptr() as *const libc::c_void, data.len())
                };
                if n < 0 { Err(std::io::Error::last_os_error()) } else { Ok(n as usize) }
            }) {
                Ok(res) => return res.map_err(|_| JkError::WriteFailed(-1)),
                Err(_would_block) => continue,
            }
        }
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let afd = self.afd.as_ref().ok_or(JkError::TransportNotInitialized)?;

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut total = 0usize;
        while total < buf.len() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { break; }

            // Wait for readiness (epoll), bounded by the overall deadline.
            let mut guard = match tokio::time::timeout(remaining, afd.readable()).await {
                Ok(Ok(g)) => g,
                Ok(Err(_)) => return Err(JkError::ReadFailed(-1)),
                Err(_) => break, // timed out
            };

            match guard.try_io(|inner| {
                let n = unsafe {
                    libc::read(inner.as_raw_fd(), buf[total..].as_mut_ptr() as *mut libc::c_void, buf.len() - total)
                };
                if n < 0 { Err(std::io::Error::last_os_error()) } else { Ok(n as usize) }
            }) {
                Ok(Ok(0)) => break,           // EOF
                Ok(Ok(n)) => total += n,
                Ok(Err(_)) => return Err(JkError::ReadFailed(-1)),
                Err(_would_block) => continue,
            }
        }
        Ok(total)
    }
}
