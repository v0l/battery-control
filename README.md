# battery_control

One async trait — `Battery` — to **monitor and control many different
batteries, BMSes and power stations** through a single normalized data model.

Devices span three loose classes (cell-level BMS, power stations, battery
monitors) so every field is optional and control is **capability-gated**:
unsupported commands return `Error::Unsupported`.

Each device family is a **feature-gated backend adapter** over an existing crate
or a native protocol decoder — you only compile (and pull the deps of) what you
use.

## Backends

| Feature | Backend | Class | Transport | Read | Control |
|---------|---------|-------|-----------|------|---------|
| `anker` *(default)* | [`anker_solix`] Anker SOLIX | power station | BLE | SOC, ports, temp | AC/DC ports |
| `jk` | [`jk_bms`] JK BMS | cell BMS | serial + BLE (async) | cells, temp, alarms | MOSFETs, settings |
| `daly` | [`dalybms`] Daly BMS | cell BMS | serial (async) | SOC, MOSFET, capacity | MOSFETs, SOC |
| `victron` | [`victron_ble`] Victron | monitor | BLE broadcast | SOC, V/I | — (read-only) |
| `pylontech-can` *(default)* | native Pylontech CAN | rack (EG4/SOK/…) | CAN frames | SOC, V/I/T, limits, alarms | — (read-only) |
| `can-socket` | Pylontech CAN via SocketCAN | rack | Linux CAN | as above | — |

`full` enables every host-buildable backend (everything except the Linux-only
`can-socket`).

[`anker_solix`]: https://crates.io/crates/anker_solix
[`jk_bms`]: https://crates.io/crates/jk_bms
[`dalybms`]: https://crates.io/crates/dalybms
[`victron_ble`]: https://crates.io/crates/victron_ble

## Example

```rust
use battery_control::{Battery, Command};
use battery_control::backends::AnkerBattery;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> battery_control::Result<()> {
    let mut bat = AnkerBattery::connect("C1000", 6).await?;

    let s = bat.status().await?;
    println!("SOC {:?}%  out {:?} W", s.soc, s.power_out);

    if bat.capabilities().contains(battery_control::Capabilities::TOGGLE_PORTS) {
        bat.execute(Command::SetPort { id: "dc".into(), on: true }).await?;
    }
    Ok(())
}
```

Because every backend implements `Battery`, you can hold heterogeneous devices
as `Box<dyn Battery>` and treat them uniformly:

```rust,ignore
let devices: Vec<Box<dyn battery_control::Battery>> = vec![
    Box::new(AnkerBattery::connect("C1000", 6).await?),
    Box::new(JkBattery::open_serial("/dev/ttyUSB0", 9600).await?),
];
for mut d in devices {
    println!("{}: {:?}%", d.info().backend, d.status().await?.soc);
}
```

The Pylontech decoder is transport-agnostic and pure — you can feed it CAN
frames from any source:

```rust,ignore
use battery_control::backends::PylontechState;
let mut s = PylontechState::new();
s.feed(0x355, &[87, 0, 100, 0, 0, 0, 0, 0]); // SOC/SOH
s.feed(0x356, &voltage_current_temp_frame);
let status = s.to_status();
```

## Data model

`BatteryStatus` normalizes across classes (`current` is **+charge / -discharge**):

- basics: `soc`, `soh`, `voltage`, `current`, `power_in/out`, `temperature_c`
- capacity: `capacity_remaining_ah`, `capacity_full_ah`, `cycles`, `time_remaining_h`
- BMS: `cells: Vec<CellInfo>`, `charging`/`discharging`, `charge/discharge_current_limit_a`
- stations: `ports: Vec<PortInfo>` — free-form ports (`id`, optional `label`, `direction` in/out/bidir, `on`, `watts`); no fixed port-type enum
- `alarms: Vec<String>`

`Command`: `SetPort`, `SetCharging`, `SetDischarging`, `SetBalancer`,
`SetChargeLimit`, `SetSetting` — each gated by `Capabilities`.

## Adding a backend

1. Add the upstream crate as an `optional` dependency and a matching feature in
   `Cargo.toml`.
2. Create `src/backends/<name>.rs` with a newtype adapter implementing
   [`Battery`]: map telemetry into `BatteryStatus`, advertise `Capabilities`,
   translate `Command`s in `execute`.
3. Re-export it from `src/backends/mod.rs` behind the feature.

Blocking upstreams should be driven with `tokio::task::block_in_place` /
`spawn_blocking`; async ones (like `jk_bms` 0.2) are awaited directly.

**Candidates to add next:** JBD/Xiaoxiang (`ubmsc`), VE.Direct serial
(`vedirect`), PACE-BMS and EG4-LL Modbus (native decoders, like Pylontech).

## License

MIT
