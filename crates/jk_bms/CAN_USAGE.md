# JK BMS CAN Transport Usage

This document describes how to use the CAN protocol transport with the JK BMS library.

## Overview

The CAN transport allows communication with JK BMS devices over a CAN bus interface. This is useful for embedded systems and industrial applications where CAN is the primary communication protocol.

## Prerequisites

1. A CAN interface (e.g., `can0`, `vcan0`)
2. Linux system with CAN support
3. Root privileges to bring up the interface

## Setting Up CAN Interface

### Using a Physical CAN Adapter

```bash
# Load the CAN driver (example for USB-CAN adapter)
sudo modprobe slcan
sudo slcand -o -s speed1000 /dev/ttyUSB0 can0

# Bring up the interface
sudo ip link set can0 up type can bitrate 250000
```

### Using Virtual CAN (for testing)

```bash
# Load virtual CAN driver
sudo modprobe vcan

# Create a virtual CAN interface
sudo ip link add dev vcan0 type vcan

# Bring up the interface
sudo ip link set up vcan0 type can bitrate 250000
```

## Usage

### Command Line Tool

```bash
# Read BMS data
jktool -t can:can0,0x18ff0000,0x18fe0000 read

# Read settings
jktool -t can:can0,0x18ff0000,0x18fe0000 settings

# Write a setting
jktool -t can:can0,0x18ff0000,0x18fe0000 set max_charge_current 50.0

# List settings
jktool list-settings
```

### Transport Format

The transport string format is:
```
can:<interface>,<rx_id>,<tx_id>
```

Where:
- `<interface>`: CAN interface name (e.g., `can0`, `/dev/can0`)
- `<rx_id>`: RX CAN ID in hexadecimal (e.g., `0x18ff0000`)
- `<tx_id>`: TX CAN ID in hexadecimal (e.g., `0x18fe0000`)

### Common CAN IDs for JK BMS

JK BMS devices typically use the following CAN IDs:
- RX (BMS to host): `0x18FF0000` (standard)
- TX (host to BMS): `0x18FE0000` (standard)

These may vary depending on your specific BMS model. Check your BMS documentation.

## CAN Protocol Details

### Frame Format

The JK BMS CAN protocol uses standard 8-byte CAN frames:

**Command Frames:**
- Byte 0: Command byte (e.g., `0x97` for info, `0x96` for cell info)
- Bytes 1-7: Command data or padding

**Response Frames:**
- Standard JK BMS protocol with frame signature `0x55 0xAA 0xEB 0x90`
- Frame size: 300 bytes (may be fragmented across multiple CAN frames)

### Supported Commands

| Command | CAN Frame | Description |
|---------|-----------|-------------|
| Get Info | `0x97` | Request device information |
| Get Cell Info | `0x96` | Request cell voltage/current data |
| Write Register | `0x05` | Write a configuration register |

### Multi-Frame Support

For responses larger than 8 bytes, the data is fragmented across multiple CAN frames. The library handles reassembly automatically using the `FrameAssembler`.

## Example: Testing with Virtual CAN

```bash
# Setup
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan
sudo ip link set up vcan0 type can bitrate 250000

# In terminal 1, start listening
sudo candump vcan0

# In terminal 2, send a test frame
cansend vcan0 18FE0000#9600000000000000

# In terminal 3, try to read (will timeout as no BMS is connected)
jktool -t can:vcan0,0x18ff0000,0x18fe0000 read
```

## Library Integration

### Rust Example

```rust
use jk_bms::{MybmmPack, MybmmModule, JkSession, jk_new, jk_open, jk_read, jk_close};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut pack = MybmmPack::new("bms1");
    pack.transport = "can".to_string();
    pack.target = "can0,0x18ff0000,0x18fe0000".to_string();
    
    let module = MybmmModule::new("jk", 0x07);
    let mut session = jk_new(pack, module)?;
    
    jk_open(&mut session)?;
    jk_read(&mut session, &mut pack)?;
    
    println!("Voltage: {:.3} V", pack.voltage);
    println!("Current: {:.3} A", pack.current);
    
    jk_close(&mut session)?;
    Ok(())
}
```

## Troubleshooting

### "Failed to get interface index"
- Ensure the CAN interface exists: `ip link show can0`
- Check interface name in transport string

### "Failed to create CAN socket"
- Check if CAN support is enabled: `lsmod | grep can`
- May need root privileges

### "Failed to set CAN filter"
- Ensure CAN interface is up: `ip link show can0`
- Check CAN ID format (should be valid hexadecimal)

### No response from BMS
- Verify CAN IDs are correct for your BMS model
- Check physical CAN bus connection
- Verify baud rate matches (typically 250kbps or 500kbps)

## Security Notes

- CAN transport requires root privileges to bring up interfaces
- Be careful when writing settings to avoid damaging the BMS
- Always verify settings before writing critical values

## References

- Linux CAN Socket API: https://www.kernel.org/doc/html/latest/networking/can.html
- JK BMS Protocol: See ESPHome JK BMS integration for detailed protocol specs
