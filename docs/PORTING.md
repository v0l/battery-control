# Porting a device: the backend guide

How to add support for a new BMS / ESS / PPS so it lands with the **same shape**
as every other backend. Two crates are the reference implementations — copy
their structure, don't invent a new one:

- [`jk_bms`](https://github.com/v0l/jktool-rs) — BLE + serial + CAN, push stream, settings, a `jktool` CLI.
- [`anker_solix`](https://github.com/v0l/anker-rs) — BLE, encrypted handshake, telemetry stream, an `anker` CLI.

The goal: a self-contained **protocol crate** (with its own CLI tool) that a thin
`battery_control` backend wraps into the `Battery` trait. Nothing device-specific
leaks into `battery_control` beyond the wrapper.

---

## 0. Decide the reuse tier first

Before writing anything, place the device in a tier (see the
[backend roadmap](https://github.com/v0l/battery-control/issues/11)):

| Tier | Strategy | You write |
|------|----------|-----------|
| **A** | An existing Rust crate covers it (`vedirect`, `victron_ble`, …) | just the `battery_control` wrapper |
| **B** | Modbus device — reuse `tokio-modbus`/`rmodbus` for framing | a register map + wrapper |
| **C** | Small custom framed protocol (like JK/JBD) | a full protocol crate |
| **D** | Crypto handshake + protobuf (EcoFlow/Jackery/…) | a full protocol crate, ported from the reference impl |

**Tier A** skips this whole guide — go to §6 (the wrapper). **B–D** create a crate.

---

## 1. Where it lives

New protocol crates are **workspace members in this repo**, not separate repos:

```
battery-control/
├── Cargo.toml            # [workspace] members = ["crates/*"]
├── src/                  # battery_control (the wrapper lib + `battery` CLI)
└── crates/
    └── <name>_bms/       # e.g. jbd_bms, sok_bms, ecoflow, …
        ├── Cargo.toml
        └── src/
            ├── lib.rs
            ├── error.rs
            ├── protocol.rs
            ├── transport/           # or transport.rs if BLE-only
            │   ├── bluetooth.rs
            │   └── serial.rs
            ├── device.rs            # the high-level handle (a.k.a. bms.rs)
            └── bin/<tool>/main.rs   # the CLI (e.g. jbdtool)
```

Naming: crate `<name>_bms` for BMSes, `<name>` for power stations (match the
vendor: `jbd_bms`, `sok_bms`, `ecoflow`, `jackery`, `bluetti`). CLI binary
`<name>tool` (`jbdtool`) or `<name>` (`ecoflow`).

---

## 2. The crate API contract

Every protocol crate exposes the **same surface** so the wrappers look identical.
Model it on `JkBms` / anker's `Device`.

```rust
// error.rs — one error enum + Result alias.
#[derive(thiserror::Error, Debug)]
pub enum Error { /* Transport(String), Protocol(String), Timeout, NotFound, … */ }
pub type Result<T> = std::result::Result<T, Error>;

// device.rs — discovery is a free function returning connectable descriptors.
pub async fn scan(secs: u64) -> Result<Vec<Discovered>>;

pub struct Discovered {              // one advertised device
    pub id: String,                  // stable peripheral id (macOS-safe) or serial path
    pub name: Option<String>,
    pub rssi: Option<i16>,
}
impl Discovered { pub async fn connect(self) -> Result<Device>; }

pub struct Device { /* transport + last decoded snapshot */ }
impl Device {
    // --- connect ---
    pub async fn connect_ble(id: &str) -> Result<Self>;        // BLE devices
    pub async fn connect_serial(target: &str) -> Result<Self>; // "<path>,<baud>" (if wired)

    // --- identity ---
    pub fn model(&self) -> &str;                               // "" until first read

    // --- read ---
    pub async fn read(&mut self) -> Result<&Data>;             // one-shot snapshot
    // Realtime: ONLY for devices that push (BLE notifications). Poll-only
    // devices (Modbus) omit this — battery_control polls read() for them.
    pub async fn next_update(&mut self, timeout: Duration) -> Result<&Data>;

    // --- control ---
    pub async fn set(&mut self, id: &str, value: &str) -> Result<()>; // "charge"="off", …

    pub async fn disconnect(&mut self) -> Result<()>;
}
```

`Data` is the crate's decoded snapshot (`MybmmPack`, anker's `Telemetry`). Keep
it a plain struct of parsed fields in real units — the wrapper maps it to the
normalized model.

**Rule:** `read()` and `next_update()` return `&Data` (borrow the cached
snapshot); mutation happens inside. This matches `JkBms`.

---

## 3. `protocol.rs` — pure, tested, no I/O

All framing and parsing lives here as **pure functions** — no transport, no
async. This is what makes it unit-testable and portable.

Provide:
- **command builders**: `fn read_x() -> [u8; N]`, `fn write_x(val) -> Vec<u8>` (checksum baked in).
- **a decoder**: `fn parse(frame: &[u8]) -> Result<Parsed>` (verify checksum/terminator).
- **a `FrameAssembler`** for BLE MTU fragmentation (frames split across notifications). Copy `jk_bms::FrameAssembler`: buffer bytes, flush on a fresh preamble, emit a frame once it's complete + checksum-valid.
- **bit/enum → string** helpers for alarms/protection flags.

**Every command builder gets a unit test against known-good bytes**, e.g. JBD:

```rust
#[test]
fn read_basic_info_frame() {
    // DD A5 03 00 FF FD 77  (checksum = 0x10000 - (0x03+0x00))
    assert_eq!(read_reg(0x03), [0xDD, 0xA5, 0x03, 0x00, 0xFF, 0xFD, 0x77]);
}
```

---

## 4. `transport/` — I/O behind a trait

Define a `Transport` trait and implement it per medium. Copy `jk_bms`'s:

```rust
#[async_trait]
pub trait Transport: Send {
    async fn open(&mut self) -> Result<()>;
    async fn close(&mut self) -> Result<()>;
    async fn write(&mut self, data: &[u8]) -> Result<usize>;
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
}
```

### BLE (`bluetooth.rs`, via `btleplug` 0.12 — match the workspace version)
- **`scan(secs)`** returns `Discovered` for peripherals that advertise the
  device's **service UUID** (JK `0xFFE0`, JBD `0xFF00`, SOK `0xFFF0`). Filter on
  the service, **not** the name — names are user-customisable. Keep a name-prefix
  fallback only as a secondary signal.
- **`BluetoothTransport`**: connect by `Peripheral::id()` (macOS-safe), discover
  services, subscribe to the **notify** characteristic, write to the **write**
  characteristic `WithoutResponse`. Assemble notifications with `FrameAssembler`.

| Device | Service | Notify | Write |
|--------|---------|--------|-------|
| JK     | ffe0    | ffe1   | ffe1  |
| JBD    | ff00    | ff01   | ff02  |
| SOK    | fff0    | fff1   | fff2  |

### Serial (`serial.rs`, via `tokio-serial`) — for wired/RS485 devices.
### CAN (`can.rs`, Linux `socketcan`) — only if the device speaks CAN.

Gate each behind a crate feature: `bluetooth` (default), `serial`, `can`.

---

## 5. `device.rs` — the read loop

One shared read routine drives every transport (this is the fix that kept
`jk_bms`'s CLI and library from drifting). Pattern:

1. write the request command,
2. read repeatedly, feeding each read through the `FrameAssembler`,
3. stop when the needed frame(s) decode; bounded retries.

Expose it as `read()` (one-shot). If the device **pushes** frames after a single
request (BLE notify), add `next_update(timeout)` that decodes the ongoing stream
without re-requesting. Poll-only devices skip it.

Add a `ScriptedTransport` test that replays fragmented reads + heartbeats and
asserts the snapshot decodes (see `jk_bms` `test_jk_read_fragmented_with_heartbeats`).

---

## 6. The `battery_control` wrapper (`src/backends/<name>.rs`)

A thin adapter — no protocol logic. Implement `Battery`:

```rust
pub struct <Name>Battery { dev: <name>::Device, info: DeviceInfo, /* cached control state */ }

#[async_trait]
impl Battery for <Name>Battery {
    fn info(&self) -> &DeviceInfo { &self.info }
    fn capabilities(&self) -> Capabilities { /* only what the device supports */ }

    async fn status(&mut self) -> Result<BatteryStatus> {
        let d = self.dev.read().await.map_err(|e| Error::Transport(e.to_string()))?;
        Ok(to_status(d))                 // map Data -> normalized model
    }

    // Push backends only:
    fn has_stream(&self) -> bool { true }
    fn stream(&mut self) -> Option<StatusStream<'_>> { /* unfold over next_update + diff */ }

    async fn execute(&mut self, cmd: Command) -> Result<()> { /* map to dev.set(...) */ }
    async fn disconnect(&mut self) -> Result<()> { self.dev.disconnect().await.map_err(…) }
}
```

- **`to_status`**: fill the normalized model via `BatteryStatus::set(Reading::…, …)`,
  `set_switch(SwitchId::…, …)`, cells, temps (`temp.t1`, `temp.mosfet`), alarms.
  See the field map in §8.
- **Poll-only devices** override nothing stream-related — `Battery::updates()`
  polls `status()` and diffs automatically.
- **Push devices** set `has_stream()==true` and implement `stream()` with the
  `futures_util::stream::unfold` + `BatteryStatus::diff` pattern (copy `jk.rs`).

### Feature flags (mirror JK)
```toml
# battery_control/Cargo.toml
<name>_bms = { path = "crates/<name>_bms", optional = true, default-features = false }

<name>       = ["<name>-ble", "<name>-serial"]        # union
<name>-ble   = ["dep:<name>_bms", "<name>_bms/bluetooth", "runtime"]
<name>-serial= ["dep:<name>_bms", "<name>_bms/serial", "runtime", "dep:tokio-serial"]
```
Add `<name>` to `full` and `cli`. Mobile builds pick `<name>-ble` only.

### Discovery wiring (`src/discovery.rs`)
- Add `Locator::<Name>Ble { id }` / `<Name>Serial { port }` and the `connect` arm.
- Add a `scan_<name>()` arm that calls `<name>::scan()` and maps to `Discovered`
  (BLE-service filtered). Run it in the concurrent `tokio::join!` in `discover()`.
- Each pack is its **own** device — never hide one behind another.

---

## 7. The CLI tool (`crates/<name>_bms/src/bin/<tool>/main.rs`)

Ship a standalone CLI matching `jktool`/`anker`. Subcommands and flags are fixed
so users learn one tool:

```
<tool> [-t <transport:target>] <SUBCOMMAND> [--format text|json] [-J]

  scan                     Discover devices (BLE service / serial probe)
  read                     Read live data once (default)
  monitor [--interval S]   Stream live data until Ctrl-C
  set <id> <value>         Control (e.g. `set charge off`)
  settings                 Dump device settings        (if supported)
  list-settings            Names/units the device accepts (if supported)

  -t, --transport   e.g. bt:<id>  |  serial:/dev/ttyUSB0,9600  |  can:can0
  --format          text (default) | json
  -J, --pretty      pretty JSON
```

The CLI **only** calls the crate's public API (`scan`, `connect_*`, `read`,
`next_update`, `set`) — it must not reach into protocol internals. Keep a small
`output.rs` for text/JSON formatting.

---

## 8. Normalized field map (`to_status`)

Map decoded fields to the shared model so the app/HA bridge render them uniformly:

| Device value | Normalized |
|--------------|-----------|
| pack voltage | `Reading::Voltage` (V) |
| pack current (+charge/−discharge) | `Reading::Current` (A); split to `PowerIn`/`PowerOut` |
| SOC / SOH | `Reading::Soc` / `Soh` (%) |
| remaining / full capacity | `Reading::CapacityRemainingAh` / `CapacityFullAh` |
| cycles | `Reading::Cycles` |
| per-cell mV (+balancing) | `BatteryStatus.cells` (`CellInfo`) |
| NTC probes | `temp.t1..` sensors (`Unit::Celsius`); MOSFET → `temp.mosfet` |
| charge/discharge FET | `SwitchId::Charging` / `Discharging` |
| balancer / heater | `SwitchId::Balancer` / `Heater` |
| protection/alarm bits | `BatteryStatus.alarms` (`Vec<String>`) |
| writable params | `BatteryStatus.settings` (`Setting`, ids match `set` names) |

Set **`writable: true`** on settings whose `id` the crate's `set()` accepts, so
they round-trip through the app and HA bridge automatically.

---

## 9. Testing & CI

- `protocol.rs`: unit-test every command builder against known bytes + checksum,
  and parse a synthetic response frame.
- `device.rs`: a `ScriptedTransport` replaying fragmented/heartbeat reads.
- The crate must `cargo test` on its own; `battery_control` must
  `cargo check --no-default-features --features <name>-ble` (and `-serial`).
- Verify every feature combo still builds (`daly`, `jk-ble`, `full`, `cli`, …).

---

## 10. Versioning & publishing

- Crate starts at `0.1.0`. Breaking changes to the public API → bump the minor
  in 0.x (e.g. adding a field to a public struct).
- In-repo crates can stay path-only; publish to crates.io if reused elsewhere
  (like `jk_bms`). Pin `battery_control` to the version/rev, not a floating range.

---

## Checklist

- [ ] Tier chosen; reference impl to port identified (from the issue).
- [ ] `crates/<name>_bms` added as a workspace member.
- [ ] `protocol.rs`: command builders + decoder + `FrameAssembler`, **unit-tested**.
- [ ] `transport/`: `Transport` trait + BLE (service-UUID scan) and/or serial/CAN.
- [ ] `device.rs`: `scan`, `connect_ble/serial`, `read`, `next_update?`, `set`, `disconnect`.
- [ ] `bin/<tool>`: `scan`/`read`/`monitor`/`set` over the public API only.
- [ ] `ScriptedTransport` read-loop test.
- [ ] `battery_control` wrapper: `Battery` impl + `to_status` field map.
- [ ] Feature flags (`<name>`, `<name>-ble`, `<name>-serial`); added to `full`/`cli`.
- [ ] Discovery arm + `Locator` + concurrent scan.
- [ ] Capabilities reflect only what the device actually supports.
- [ ] All feature combos build; crate tests pass.
