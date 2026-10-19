# Changelog

All notable changes to battery control are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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
