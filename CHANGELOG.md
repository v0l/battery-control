# Changelog

All notable changes to battery control are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.2] - 2026-11-25

### Fixed
- The window opens with the wgpu renderer and checks at startup that it draws, so it no longer opens blank on machines where the OpenGL renderer fails.
- The jktool example builds again, and the Pylontech example only builds with the pylontech-cli feature.

### Changed
- Every release now includes a source archive with its checksum.

## [0.1.1] - 2026-10-31

### Changed
- egui and eframe 0.37, which keeps the window running on Wayland when its workspace is hidden.

## [0.1.0] - 2026-10-19

### Added
- A desktop window (`battery-control`) over the same `Battery` trait. It scans Bluetooth, and
  USB serial ports when asked, then shows charge, voltage, current, power, every cell with its
  balancing flag, temperatures, ports and alarms, with a history plot.
- Switches the charge and discharge MOSFETs, the balancer and the output ports, writes settings
  (toggles, pickers and bounded number fields) and runs the pairing flow for devices that need
  it. Pairing starts on connect.
- Readings stream in as the device pushes them where the backend can, and are polled every two
  seconds where it cannot. Switches change on click and settle when the device confirms.
- `--demo` opens a made-up 16 cell pack, and `--version` prints the version, which the bench
  suite reads.
- Releases carry `.tar.gz` and `.zip` archives with sha256 files, a macOS `.dmg` and a Windows
  `.msi` for each target.

### Fixed
- Anker SOLIX C1000 Gen 2: the AC and DC output switches go through the secure channel after
  pairing. Sent over the plain channel the device acknowledged them and did nothing.
- Anker SOLIX: a settings or output write returns at once. The telemetry subscribes that follow
  are sent by the update stream instead of the write sleeping for them.
