# jk_bms / jktool

[![crates.io](https://img.shields.io/crates/v/jk_bms.svg)](https://crates.io/crates/jk_bms)
[![docs.rs](https://docs.rs/jk_bms/badge.svg)](https://docs.rs/jk_bms)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Async Rust implementation of [jktool](https://github.com/sshoecraft/jktool) — a library and command-line tool for communicating with JIKONG (JK) Battery Management Systems.

Supports the JK02 (24S/32S) and JK04 protocol variants, with automatic detection based on the BMS model string. Provides a reusable async library (`jk_bms`) and a CLI (`jktool`).

## Features

- **Async-first**: `Transport` trait and session API are fully `async` on a Tokio runtime
- **Multi-transport**: Serial and CAN (Linux), Bluetooth LE (cross-platform), with the same `transport:target` syntax as the original
- **Readiness-based I/O**: raw fds are driven by Tokio's reactor (`AsyncFd`); BLE consumes the notification `Stream` directly — no busy polling
- **Protocol support**: JK02_24S, JK02_32S (PB2/BD/HY series), JK04 (auto-detected from info frame)
- **Live data**: cell voltages, temperatures, SOC/SOH, power, errors, MOSFET states
- **Settings read/write**: full register map with human-readable names, scaling, and write-frame generation
- **Output formats**: text, CSV, JSON (with pretty-print)
- **BLE frame assembly**: reassembles 300-byte JK frames from small BLE MTU chunks with CRC verification

## Platform support

| Platform | Bluetooth LE | Serial | CAN |
|----------|:---:|:---:|:---:|
| Linux    | ✅ | ✅ | ✅ |
| macOS    | ✅ | — | — |
| Windows  | ✅ | — | — |

Serial and CAN use Linux-specific APIs (termios / SocketCAN) and are compiled only on Linux.

## Install

CLI:

```bash
cargo install jk_bms
```

Library:

```bash
cargo add jk_bms
```

The `bluetooth` feature is enabled by default. On Linux it links `libdbus` (`sudo apt install libdbus-1-dev`). To build without it:

```bash
cargo add jk_bms --no-default-features            # core protocol only
cargo add jk_bms --no-default-features --features can
```

## CLI usage

Transports are specified as `transport:target[,options]` — same syntax as the original jktool.

```bash
# Read live data (default action)
jktool -t serial:/dev/ttyUSB0,9600
jktool -t bt:01:02:03:04:05:06,ffe1
jktool -t can:can0,0x18ff0000,0x18fe0000

# Read settings
jktool -t bt:01:02:03:04:05:06 settings

# Write a setting
jktool -t serial:/dev/ttyUSB0,9600 set max_charge_current 50.0
jktool -t bt:01:02:03:04:05:06 set charging on

# List supported settings
jktool list-settings

# Output to file as JSON
jktool -t serial:/dev/ttyUSB0,9600 -f json -o pack.json

# Scan for Bluetooth devices
jktool scan

# Scan for JK BMS devices on a CAN bus (Linux)
jktool scan-can -i can0
```

## Library usage

The `jk_bms` crate exposes the async `Transport`/`JkSession` API as well as the pure
protocol codec (`getdata`, `FrameAssembler`, command builders), which is transport-agnostic
and has no async requirements.

### Decoding frames (no transport)

```rust
use jk_bms::{MybmmPack, FrameAssembler, getdata};

let mut pack = MybmmPack::new("pack1");
let mut assembler = FrameAssembler::new();

// Feed raw bytes from any source; decodes once a full CRC-valid frame is buffered.
if let Some(flags) = assembler.feed_and_decode(&mut pack, &bytes) {
    if flags.got_volts {
        println!("Voltage: {:.3} V", pack.voltage);
        println!("Cells:   {}", pack.cells);
    }
}
```

### Async transport

Implement the async `Transport` trait for your link, then drive a session:

```rust
use jk_bms::{Transport, Result, async_trait};

struct MyTransport { /* ... */ }

#[async_trait]
impl Transport for MyTransport {
    async fn open(&mut self) -> Result<()> { Ok(()) }
    async fn close(&mut self) -> Result<()> { Ok(()) }
    async fn write(&mut self, data: &[u8]) -> Result<usize> { Ok(data.len()) }
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> { Ok(0) }
}
```

## Protocol versions

| Version | Models | Cell voltages | Max cells |
|---------|--------|---------------|-----------|
| JK02_24S | JK-B2AxxS | 2-byte LE mV | 24 |
| JK02_32S | JK_PB2, JK-BD, JK_HY | 2-byte LE mV (16-byte offset) | 32 |
| JK04 | Older JK-BMS | IEEE 754 float | 24 |

Auto-detected from the BMS info frame model string.

## License

MIT
