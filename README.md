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
| `jbd` | [`jbd_bms`] JBD / Xiaoxiang / Overkill | cell BMS | serial + BLE | cells, temp, alarms | charge/discharge MOSFETs |
| `sok` | [`sok_bms`] SOK / ABC-BMS | cell BMS | BLE (EE + Modbus) | cells, temps, SOC, capacity | — (read-only) |
| `renogy` | [`renogy_bms`] Renogy smart battery | cell BMS | BLE (BT-1/BT-2) | cells, temps, SOC, capacity | — (read-only) |
| `pace` | [`pace_bms`] PACE-BMS (many rack rebrands) | rack BMS | RS485 (Modbus) | cells, temps, SOC/SOH, capacity | — (read-only) |
| `seplos` | [`seplos_bms`] Seplos V3 | rack BMS | RS485 (Modbus) | 16 cells, temps, SOC/SOH, capacity | — (read-only) |
| `bluetti` | [`bluetti`] Bluetti (AC/EB/EP) | power station | BLE (local, Modbus) | SOC, in/out power, ports, cells | AC/DC output |
| `daly` | [`dalybms`] Daly BMS | cell BMS | serial (async) | SOC, MOSFET, capacity | MOSFETs, SOC |
| `victron` | [`victron_ble`] Victron | monitor | BLE broadcast | SOC, V/I, temp, alarms | — (read-only) |
| `vedirect` | [`vedirect`] Victron VE.Direct | monitor | serial (BMV/SmartShunt) | V/I, SOC, TTG | — (read-only) |
| `pylontech-can` *(default)* | native Pylontech CAN | rack (EG4/SOK/…) | CAN frames | SOC, V/I/T, limits, alarms | — (read-only) |
| `can-socket` | Pylontech CAN via SocketCAN | rack | Linux CAN | as above | — |

`full` enables every host-buildable backend (everything except the Linux-only
`can-socket`).

[`anker_solix`]: https://crates.io/crates/anker_solix
[`jk_bms`]: https://crates.io/crates/jk_bms
[`jbd_bms`]: ./crates/jbd_bms
[`sok_bms`]: ./crates/sok_bms
[`renogy_bms`]: ./crates/renogy_bms
[`pace_bms`]: ./crates/pace_bms
[`seplos_bms`]: ./crates/seplos_bms
[`bluetti`]: ./crates/bluetti
[`dalybms`]: https://crates.io/crates/dalybms
[`victron_ble`]: https://crates.io/crates/victron_ble
[`vedirect`]: https://crates.io/crates/vedirect

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

See **[docs/PORTING.md](docs/PORTING.md)** for the full process — it defines the
canonical crate API + CLI shape (matching `jk_bms`/`anker_solix`), the workspace
layout for ported protocol crates (`crates/<name>_bms`), transport/BLE
conventions, the `Battery` wrapper, the field map, and a checklist.

The short version:

1. Pick a reuse tier (A: existing crate · B: Modbus + register map · C: custom
   protocol · D: crypto port) — see the [backend roadmap](https://github.com/v0l/battery-control/issues/11).
2. For B–D, add a `crates/<name>_bms` workspace crate with `protocol.rs` /
   `transport/` / `device.rs` / a `<name>tool` CLI — same surface as `jk_bms`.
3. Add `src/backends/<name>.rs`: a newtype adapter implementing [`Battery`]
   (map telemetry into `BatteryStatus`, advertise `Capabilities`, translate
   `Command`s), re-export from `src/backends/mod.rs`, and wire discovery.

Progress is tracked in the [backend roadmap](https://github.com/v0l/battery-control/issues/11)
(JBD, SOK, Renogy, Seplos, PACE, Pylontech RS485, Victron, EcoFlow, Jackery, Bluetti).

## Home Assistant

[battery-ha-bridge](https://github.com/v0l/battery-ha-bridge) exposes every
connected battery to Home Assistant via MQTT Discovery — sensors,
capability-gated switches and a charge-limit number entity appear
automatically. It also ships as a Home Assistant OS add-on.

## License

MIT
