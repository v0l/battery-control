# anker-rs

A small, dependency-light Rust **library** (`anker_solix`) and **CLI** (`anker`)
for talking to **Anker SOLIX** portable power stations directly over
**Bluetooth LE**.

> ⚠️ **For educational and research purposes only.** Independent project,
> **not** affiliated with or endorsed by Anker. Provided "AS IS" with no
> warranty — see [Legal & disclaimer](#legal--disclaimer) before use.

---

## Features

- **Scan** for nearby SOLIX stations and identify the model from its BLE name.
- **Read telemetry** — battery %, charge/discharge limits, temperature, total
  power in/out, and per-port status + watts (AC, DC 12 V, solar, USB-C/A).
- **Control outputs** — toggle the AC and DC rails (and, via the library, the
  LCD display and ambient light).
- **Session security** — performs the station's ECDH (secp256r1) key agreement
  and AES-128 encrypted framing, including the gen-2 AES-GCM channel.
- **Two front-ends** — a `anker` command-line tool and an embeddable async
  library.
- **Text or JSON** output.

## Supported hardware

| Model | Status |
|-------|--------|
| SOLIX **C1000 Gen 2** (A1763) | Tested on real hardware — telemetry + AC/DC control |
| C300 / C800 / C1000 gen 1, F2000, F3800 | Implemented (gen-1 framing); not independently verified |

The model is auto-detected from the advertised BLE name.

## Install

Requires a recent stable Rust toolchain (edition 2024, Rust ≥ 1.85).

```bash
git clone https://github.com/v0l/anker-rs
cd anker-rs
cargo build --release
# binary at target/release/anker
```

## CLI

```bash
# list nearby stations
anker scan

# one telemetry snapshot (defaults to targeting a name containing "C1000")
anker status
anker -d "C1000" status
anker -f json status

# stream telemetry until Ctrl-C
anker monitor
anker monitor --interval 5

# control outputs
anker ac on
anker dc off
```

Global flags:

| Flag | Meaning |
|------|---------|
| `-d, --device <name\|mac>` | Target a device by BLE name substring or MAC |
| `--scan-secs <n>` | How long to scan when locating a device |
| `-f, --format <text\|json>` | Output format |
| `-v`, `-vv` | Increase log verbosity |

## Library

```rust
use anker_solix::Device;
use std::time::Duration;

#[tokio::main]
async fn main() -> anker_solix::Result<()> {
    // Find a station whose BLE name contains "C1000" and connect.
    let mut dev = Device::find_and_connect("C1000", 6).await?;

    // Read a telemetry snapshot.
    let t = dev.next_telemetry(Duration::from_secs(10)).await?;
    println!("battery: {:?}%", t.battery_percentage);

    // Toggle the DC output.
    dev.set_dc(true).await?;
    Ok(())
}
```

Key entry points: [`Device::scan`], [`Device::find_and_connect`],
[`Device::next_telemetry`], and the `set_ac` / `set_dc` / `set_display` /
`set_light` controls. Add `anker_solix` as a path or git dependency in your
`Cargo.toml`.

## How it works

The station exposes a small GATT service. In brief:

1. **Discover** — scan for the advertised identifier service (`0000ff09-…`).
2. **Negotiate** — a fixed handshake on the command characteristic performs an
   ECDH (secp256r1) key agreement, yielding a session key used for AES framing.
3. **Session** — telemetry arrives on the notify characteristic as length-framed
   packets, reassembled from fragments and decrypted into TLV parameters;
   commands are sent back the same way.

## Platform notes

- **macOS:** grant your terminal app Bluetooth permission
  (System Settings → Privacy & Security → Bluetooth) or scans return nothing.
  macOS also masks device MAC addresses, so target devices by **name**.
- Only one BLE connection to a station at a time — close the official app first.

## Credits

Builds on prior community work in
[flip-dots/SolixBLE](https://github.com/flip-dots/SolixBLE) (MIT).

## Legal & disclaimer

For **educational and research purposes only**. Independent project, **not**
affiliated with or endorsed by Anker; "Anker" and "SOLIX" are trademarks of
their respective owners, used only to identify compatible hardware. Contains no
Anker code, firmware, or assets. Provided **"AS IS" with no warranty** — BLE
interaction may change settings, damage the device, or void its warranty, and
you use it entirely at your own risk.

## License

[MIT](LICENSE)
