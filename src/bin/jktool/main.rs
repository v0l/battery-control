use clap::{Parser, Subcommand, ValueEnum};
use jk_bms::{MybmmPack, MybmmModule, Transport, JkInfo, ProtocolVersion};
use jk_bms::{SETTINGS, build_setting_write_frame, build_can_setting_write_frame};

#[cfg(feature = "serial")]
use jk_bms::transport::SerialTransport;
#[cfg(all(feature = "can", target_os = "linux"))]
use jk_bms::transport::CanTransport;

#[cfg(feature = "bluetooth")]
use jk_bms::transport::BluetoothTransport;

#[derive(Parser, Debug)]
#[command(name = "jktool")]
#[command(about = "JK BMS command-line tool")]
struct Cli {
    /// Transport:target, e.g. serial:/dev/ttyUSB0,9600 or bt:01:02:03:04:05:06,ffe1 or can:can0,0x18ff0000,0x18fe0000
    #[arg(short, long)]
    transport: Option<String>,

    /// Output file
    #[arg(short, long)]
    output: Option<String>,

    /// Output format
    #[arg(short, long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,

    /// Pretty-print JSON
    #[arg(short = 'J', long)]
    pretty: bool,

    /// Debug level (0-1: hex dump, 2+: more verbose)
    #[arg(short, long, default_value_t = 0)]
    debug: u8,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Clone, Debug, ValueEnum)]
enum OutputFormat {
    Text,
    Csv,
    Json,
}

#[derive(Clone, Debug, Subcommand)]
enum Commands {
    /// Read BMS live data (default action)
    Read,

    /// Read BMS settings/configuration
    Settings,

    /// Write a setting value to the BMS
    Set {
        /// Setting name (e.g. max_charge_current, cell_ovp)
        name: String,
        /// Value to write (e.g. 50.0, on, off, true, false)
        value: String,
    },

    /// List all supported settings for the current protocol version
    ListSettings,

    /// Scan for Bluetooth devices
    #[cfg(feature = "bluetooth")]
    Scan,

    /// Scan for JK BMS devices on CAN bus
    #[cfg(target_os = "linux")]
    ScanCan {
        /// CAN interface to use (e.g., can0)
        #[arg(short, long, default_value = "can0")]
        interface: String,
        /// Timeout in seconds for scanning
        #[arg(short, long, default_value = "5")]
        timeout: u64,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    #[cfg(feature = "bluetooth")]
    if matches!(cli.command, Some(Commands::Scan)) {
        println!("Scanning for Bluetooth devices...");
        match jk_bms::transport::scan().await {
            Ok(devices) => {
                if devices.is_empty() {
                    println!("No Bluetooth devices found.");
                } else {
                    let banks = jk_bms::group_banks(devices);
                    println!("Found {} device(s):", banks.len());
                    let fmt_rssi = |r: Option<i16>| {
                        r.map(|r| format!("{} dBm", r)).unwrap_or_else(|| "N/A".to_string())
                    };
                    for bank in &banks {
                        let m = &bank.master;
                        let name = m.name.as_deref().unwrap_or("Unknown");
                        let tag = if bank.pack_count() > 1 {
                            format!(" [bank of {}]", bank.pack_count())
                        } else {
                            String::new()
                        };
                        println!("  {:30} {:38} RSSI: {}{}", name, m.id, fmt_rssi(m.rssi), tag);
                        for s in &bank.slaves {
                            let sname = s.name.as_deref().unwrap_or("Unknown");
                            let addr = s.address.map(|a| format!("#{a:02}")).unwrap_or_default();
                            println!(
                                "    └ slave {:24} {:38} RSSI: {} (read via master)",
                                format!("{sname} {addr}"),
                                s.id,
                                fmt_rssi(s.rssi)
                            );
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("Bluetooth scan failed: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    let command = cli.command.clone().unwrap_or(Commands::Read);

    // List-settings doesn't need a transport
    if matches!(command, Commands::ListSettings) {
        list_settings(&cli);
        return;
    }

    // CAN scan doesn't need a full session
    #[cfg(target_os = "linux")]
    if matches!(command, Commands::ScanCan { .. }) {
        handle_can_scan(&cli, &command);
        return;
    }

    let transport_str = cli.transport.as_deref().unwrap_or("serial:/dev/ttyUSB0,9600");

    if cli.debug > 0 {
        eprintln!("transport: {}", transport_str);
    }

    let (transport_name, target) = parse_transport(transport_str);

    let mut pack = MybmmPack::new("pack1");
    pack.transport = transport_name.to_string();
    pack.target = target.to_string();

    let module = MybmmModule::new("jk", 0x07);

    let mut session = match create_session(&pack, &module) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to create session: {}", e);
            std::process::exit(1);
        }
    };

    if let Err(e) = session.open().await {
        eprintln!("Failed to open transport: {}", e);
        std::process::exit(1);
    }

    match command {
        Commands::Read => {
            let info = do_read(&mut session, &mut pack, &cli, false).await;
            let output = format_output(&info, &pack, &cli);
            write_output(&output, &cli);
        }
        Commands::Settings => {
            let _info = do_read(&mut session, &mut pack, &cli, true).await;
            if let Some(ref settings) = pack.settings {
                let output = format_settings(settings, &pack.protocol_version, &cli);
                write_output(&output, &cli);
            } else {
                eprintln!("No settings frame received from BMS");
                let _ = session.close().await;
                std::process::exit(1);
            }
            let _ = session.close().await;
            return;
        }
        Commands::Set { name, value } => {
            // First do a read to detect protocol version
            let _info = do_read(&mut session, &mut pack, &cli, false).await;

            // Determine if using CAN transport
            let is_can = pack.transport == "can";
            
            let frame: Vec<u8> = if is_can {
                match build_can_setting_write_frame(&name, &value, pack.protocol_version) {
                    Some(f) => f.to_vec(),
                    None => {
                        eprintln!("Unknown or unsupported setting: '{}' for protocol {:?}", name, pack.protocol_version);
                        eprintln!("Use 'list-settings' to see supported settings.");
                        let _ = session.close().await;
                        std::process::exit(1);
                    }
                }
            } else {
                match build_setting_write_frame(&name, &value, pack.protocol_version) {
                    Some(f) => f.to_vec(),
                    None => {
                        eprintln!("Unknown or unsupported setting: '{}' for protocol {:?}", name, pack.protocol_version);
                        eprintln!("Use 'list-settings' to see supported settings.");
                        let _ = session.close().await;
                        std::process::exit(1);
                    }
                }
            };

            if cli.debug > 0 {
                eprintln!("DEBUG: write frame for {}={}: {:02X?}", name, value, frame);
            }

            if let Some(ref mut handle) = session.tp_handle {
                match handle.write(&frame).await {
                    Ok(n) => {
                        println!("Wrote {} bytes: {} = {}", n, name, value);
                        // Read response to confirm
                        let mut buf = vec![0u8; 2048];
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        match handle.read(&mut buf).await {
                            Ok(bytes) if bytes > 0 => {
                                if cli.debug > 0 {
                                    eprintln!("DEBUG: response {} bytes", bytes);
                                }
                                println!("Setting written successfully.");
                            }
                            Ok(_) => {
                                println!("Setting written (no response confirmation).");
                            }
                            Err(e) => {
                                eprintln!("Warning: no response after write: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Write failed: {}", e);
                        let _ = session.close().await;
                        std::process::exit(1);
                    }
                }
            }
        }
        #[cfg(feature = "bluetooth")]
        Commands::Scan => { unreachable!() }
        #[cfg(target_os = "linux")]
        Commands::ScanCan { .. } => { unreachable!() }
        Commands::ListSettings => { unreachable!() }
    }

    let _ = session.close().await;
}

async fn do_read(session: &mut jk_bms::JkSession, pack: &mut MybmmPack, cli: &Cli, need_settings: bool) -> JkInfo {
    // The read loop lives in the library (shared with `JkBms::read`).
    if let Err(e) = jk_bms::read_data(session, pack, need_settings).await {
        eprintln!("Warning: {}", e);
    }
    let _ = cli;
    JkInfo::from_pack(pack)
}

#[cfg(target_os = "linux")]
fn handle_can_scan(_cli: &Cli, command: &Commands) {
    let timeout = if let Commands::ScanCan { timeout, .. } = command {
        *timeout
    } else {
        5
    };

    let interface = if let Commands::ScanCan { interface, .. } = command {
        interface.clone()
    } else {
        "can0".to_string()
    };

    println!("Scanning for JK BMS devices on {}...", interface);
    println!("Timeout: {} seconds", timeout);
    println!();

    // Common JK BMS CAN broadcast ID (0x18FE0000 is typical for requests)
    // We'll try multiple common IDs and listen for responses
    let broadcast_ids = [
        0x18FE0000, // Standard broadcast request
        0x18FF0000, // Alternative broadcast
        0x00000000, // Zero ID (some devices)
    ];

    let discovered_devices = scan_can_bus(&interface, &broadcast_ids, timeout);

    if discovered_devices.is_empty() {
        println!("No JK BMS devices found on CAN bus.");
        println!();
        println!("Tips:");
        println!("  - Ensure CAN interface is up: sudo ip link set {} up", interface);
        println!("  - Check CAN bitrate: sudo ip link show {}", interface);
        println!("  - Verify physical CAN bus connection");
    } else {
        println!("Found {} JK BMS device(s):", discovered_devices.len());
        println!();
        for (i, device) in discovered_devices.iter().enumerate() {
            println!("  Device {}:", i + 1);
            println!("    RX ID (BMS->Host): 0x{:08X}", device.rx_id);
            println!("    TX ID (Host->BMS): 0x{:08X}", device.tx_id);
            if !device.model.is_empty() {
                println!("    Model: {}", device.model);
            }
            if !device.hwvers.is_empty() {
                println!("    Hardware: {}", device.hwvers);
            }
            if !device.swvers.is_empty() {
                println!("    Software: {}", device.swvers);
            }
            println!();
        }
        println!("Usage:");
        for device in &discovered_devices {
            println!(
                "  jktool -t can:{},0x{:08X},0x{:08X} read",
                interface, device.rx_id, device.tx_id
            );
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
struct CanDevice {
    rx_id: u32,
    tx_id: u32,
    model: String,
    hwvers: String,
    swvers: String,
}

#[cfg(target_os = "linux")]
fn scan_can_bus(interface: &str, broadcast_ids: &[u32], timeout_secs: u64) -> Vec<CanDevice> {
    use std::process::Command;
    use std::time::Duration;

    // Common JK BMS response ID patterns
    // JK BMS typically uses paired IDs: 0x18FE0000 (TX) <-> 0x18FF0000 (RX)
    let _known_pairs = [
        (0x18FF0000, 0x18FE0000), // Standard pair
        (0x18FF0001, 0x18FE0001), // Alternative
        (0x00000000, 0x00000001), // Generic
    ];

    let mut discovered = Vec::new();
    let mut seen_rx_ids = std::collections::HashSet::new();

    // Try to bring up the interface
    let _ = Command::new("ip")
        .args(["link", "set", interface, "up"])
        .output();

    // Create CAN socket
    let fd = unsafe {
        libc::socket(libc::AF_CAN, libc::SOCK_RAW, libc::CAN_RAW)
    };

    if fd < 0 {
        eprintln!("Failed to create CAN socket: {}", std::io::Error::last_os_error());
        return discovered;
    }

    // Get interface index
    let if_index = unsafe {
        let mut ifreq: libc::ifreq = std::mem::zeroed();
        let name_bytes = interface.as_bytes();
        for (i, &b) in name_bytes.iter().enumerate() {
            if i < libc::IFNAMSIZ {
                ifreq.ifr_name[i] = b as _;
            }
        }

        let test_fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
        if test_fd < 0 {
            -1
        } else {
            let ret = libc::ioctl(test_fd, libc::SIOCGIFINDEX, &mut ifreq);
            libc::close(test_fd);
            if ret < 0 {
                -1
            } else {
                ifreq.ifr_ifru.ifru_ifindex
            }
        }
    };

    if if_index < 0 {
        eprintln!("Failed to get interface index for {}", interface);
        unsafe { libc::close(fd); };
        return discovered;
    }

    // Connect to interface using bind instead of connect
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
        eprintln!("Failed to bind CAN socket: {}", std::io::Error::last_os_error());
        unsafe { libc::close(fd); };
        return discovered;
    }

    // Set non-blocking
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL, 0) };
    unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };

    // Set RX filter to receive all messages
    let filter = libc::can_filter {
        can_id: 0,
        can_mask: 0,
    };
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_CAN_BASE,
            libc::CAN_RAW_FILTER,
            &filter as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::can_filter>() as libc::socklen_t,
        )
    };

    #[repr(C)]
    struct CanFrame {
        can_id: u32,
        data: [u8; 8],
        len: u8,
    }

    // Send broadcast requests
    for &bcast_id in broadcast_ids {
        if bcast_id == 0 {
            continue;
        }

        let cmd_frame = get_can_info_command();
        let frame = CanFrame {
            can_id: bcast_id,
            data: cmd_frame,
            len: 8,
        };

        let _ = unsafe {
            libc::write(
                fd,
                &frame as *const _ as *const libc::c_void,
                std::mem::size_of::<CanFrame>(),
            )
        };

        // Small delay between broadcasts
        std::thread::sleep(Duration::from_millis(100));
    }

    // Listen for responses
    let start = std::time::Instant::now();
    let _response_buffer = vec![0u8; 2048];

    while start.elapsed() < Duration::from_secs(timeout_secs) {
        let mut frame = CanFrame {
            can_id: 0,
            data: [0u8; 8],
            len: 0,
        };

        let bytes_read = unsafe {
            libc::read(
                fd,
                &mut frame as *mut _ as *mut libc::c_void,
                std::mem::size_of::<CanFrame>(),
            )
        };

        if bytes_read > 0 && frame.len > 0 {
            // Check if this is a JK BMS response (starts with 0x55 0xAA 0xEB 0x90)
            if frame.len >= 4
                && frame.data[0] == 0x55
                && frame.data[1] == 0xAA
                && frame.data[2] == 0xEB
                && frame.data[3] == 0x90
            {
                let rx_id = frame.can_id;
                if !seen_rx_ids.contains(&rx_id) {
                    seen_rx_ids.insert(rx_id);

                    // Calculate TX ID (typically RX - 0x1000000 or similar pattern)
                    let tx_id = if rx_id >= 0x1000000 {
                        rx_id - 0x1000000
                    } else {
                        rx_id + 1 // Fallback
                    };

                    // Try to get device info by sending a request
                    let info_cmd = get_can_info_command();
                    let req_frame = CanFrame {
                        can_id: tx_id,
                        data: info_cmd,
                        len: 8,
                    };

                    let _ = unsafe {
                        libc::write(
                            fd,
                            &req_frame as *const _ as *const libc::c_void,
                            std::mem::size_of::<CanFrame>(),
                        )
                    };

                    // Wait for response and parse
                    std::thread::sleep(Duration::from_millis(200));

                    let mut info_buf = vec![0u8; 2048];
                    let info_read = unsafe {
                        libc::read(
                            fd,
                            info_buf.as_mut_ptr() as *mut libc::c_void,
                            info_buf.len(),
                        )
                    };

                    let mut model = String::new();
                    let mut hwvers = String::new();
                    let mut swvers = String::new();

                    if info_read > 0 {
                        // Try to parse as JK info frame
                        let mut pack = jk_bms::MybmmPack::new("scan");
                        let flags = jk_bms::getdata(&mut pack, &info_buf[..info_read as usize]);
                        if flags.got_info && !pack.model.is_empty() {
                            model = pack.model.clone();
                            hwvers = pack.hwvers.clone();
                            swvers = pack.swvers.clone();
                        }
                    }

                    discovered.push(CanDevice {
                        rx_id,
                        tx_id,
                        model,
                        hwvers,
                        swvers,
                    });

                    println!("  Found device at RX=0x{:08X}, TX=0x{:08X}", rx_id, tx_id);
                }
            }
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    unsafe { libc::close(fd); };
    discovered
}

fn list_settings(cli: &Cli) {
    // Default to JK02_32S if no protocol version specified
    let version = ProtocolVersion::Jk02_32S;
    let idx = match version {
        ProtocolVersion::Jk04 => 0,
        ProtocolVersion::Jk02_24S => 1,
        ProtocolVersion::Jk02_32S => 2,
    };

    match cli.format {
        OutputFormat::Json => {
            let entries: Vec<serde_json::Value> = SETTINGS.iter()
                .filter(|s| s.registers[idx] != 0)
                .map(|s| serde_json::json!({
                    "name": s.name,
                    "unit": s.unit,
                    "register": format!("0x{:02X}", s.registers[idx]),
                    "factor": s.factor,
                    "length": s.length,
                    "is_switch": s.is_switch,
                }))
                .collect();
            let json = serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_string());
            println!("{}", json);
        }
        OutputFormat::Csv => {
            println!("name,unit,register,factor,length,is_switch");
            for s in SETTINGS.iter().filter(|s| s.registers[idx] != 0) {
                println!("{},{},0x{:02X},{},{},{}", s.name, s.unit, s.registers[idx], s.factor, s.length, s.is_switch);
            }
        }
        OutputFormat::Text => {
            println!("{:30} {:>6} {:>4} {:>8} {:>3} Switch", "Name", "Unit", "Reg", "Factor", "Len");
            println!("{}", "-".repeat(60));
            for s in SETTINGS.iter().filter(|s| s.registers[idx] != 0) {
                println!("{:30} {:>6} 0x{:02X} {:>8.0} {:>3} {}",
                    s.name, s.unit, s.registers[idx], s.factor, s.length,
                    if s.is_switch { "yes" } else { "" });
            }
        }
    }
}

fn parse_transport(spec: &str) -> (&str, &str) {
    if let Some(pos) = spec.find(':') {
        (&spec[..pos], &spec[pos + 1..])
    } else {
        ("serial", spec)
    }
}

fn create_session(pack: &MybmmPack, _module: &MybmmModule) -> jk_bms::Result<jk_bms::JkSession> {
    let transport: Box<dyn Transport> = match pack.transport.as_str() {
        #[cfg(feature = "serial")]
        "serial" => Box::new(SerialTransport::from_target(&pack.target)),
        #[cfg(all(feature = "can", target_os = "linux"))]
        "can" => Box::new(CanTransport::from_target(&pack.target)?),
        #[cfg(feature = "bluetooth")]
        "bt" => Box::new(BluetoothTransport::from_target(&pack.target)),
        _ => {
            return Err(jk_bms::JkError::TransportError(
                format!("Unsupported transport: {}", pack.transport)
            ));
        }
    };

    let session = jk_bms::JkSession {
        pp: pack.clone(),
        tp: _module.clone(),
        tp_handle: Some(transport),
    };
    Ok(session)
}

fn write_output(output: &str, cli: &Cli) {
    if let Some(path) = &cli.output {
        if let Err(e) = std::fs::write(path, output) {
            eprintln!("Failed to write {}: {}", path, e);
            std::process::exit(1);
        }
    } else {
        println!("{}", output.trim_end());
    }
}

fn format_output(info: &JkInfo, pack: &MybmmPack, cli: &Cli) -> String {
    match cli.format {
        OutputFormat::Json => format_json(info, pack, cli.pretty),
        OutputFormat::Csv => format_csv(info, pack),
        OutputFormat::Text => format_text(info, pack),
    }
}

fn format_text(info: &JkInfo, pack: &MybmmPack) -> String {
    let mut out = String::new();
    out.push_str(&format!("{:25} {:.3} V\n", "Voltage:", pack.voltage));
    out.push_str(&format!("{:25} {:.3} A\n", "Current:", pack.current));
    out.push_str(&format!("{:25} {:.3} W\n", "Power:", pack.power));
    out.push_str(&format!("{:25} {:.3} W\n", "Charging Power:", pack.charging_power));
    out.push_str(&format!("{:25} {:.3} W\n", "Discharging Power:", pack.discharging_power));
    out.push_str(&format!("{:25} {}\n", "Cells:", pack.cells));
    for i in 0..pack.cells as usize {
        out.push_str(&format!("{:25} {:.3} V  {:.3} mΩ\n",
            format!("  Cell {}:", i + 1), pack.cellvolt[i], pack.cellres[i]));
    }
    if pack.cells > 0 {
        out.push_str(&format!("{:25} {:.3} V\n", "  Cell Min:", info.cell_min));
        out.push_str(&format!("{:25} {:.3} V\n", "  Cell Max:", info.cell_max));
        out.push_str(&format!("{:25} {:.3} V\n", "  Cell Diff:", info.cell_diff));
        out.push_str(&format!("{:25} {:.3} V\n", "  Cell Avg:", info.cell_avg));
    }
    out.push_str(&format!("{:25} {:.1} °C\n", "MOSFET Temp:", pack.power_tube_temp));
    out.push_str(&format!("{:25} {}\n", "Temp Probes:", pack.ntemps));
    for i in 0..pack.ntemps as usize {
        out.push_str(&format!("{:25} {:.1} °C\n", format!("  Temp {}:", i + 1), pack.temps[i]));
    }
    out.push_str(&format!("{:25} {:.1} %\n", "SOC:", pack.soc));
    out.push_str(&format!("{:25} {:.1} %\n", "SOH:", pack.soh));
    out.push_str(&format!("{:25} {:.3} Ah\n", "Capacity Remaining:", pack.capacity_remaining));
    out.push_str(&format!("{:25} {:.3} Ah\n", "Total Capacity:", pack.total_battery_capacity));
    out.push_str(&format!("{:25} {}\n", "Charging Cycles:", pack.charging_cycles));
    out.push_str(&format!("{:25} {:.3} A\n", "Balancing Current:", pack.balancing_current));
    out.push_str(&format!("{:25} {}\n", "Balancing:", if pack.balancing { "Active" } else { "Off" }));
    out.push_str(&format!("{:25} {}\n", "Charging MOS:", if pack.charging { "On" } else { "Off" }));
    out.push_str(&format!("{:25} {}\n", "Discharging MOS:", if pack.discharging { "On" } else { "Off" }));
    out.push_str(&format!("{:25} {}\n", "Precharging:", if pack.precharging { "On" } else { "Off" }));
    out.push_str(&format!("{:25} {}\n", "Heating:", if pack.heating { "On" } else { "Off" }));
    out.push_str(&format!("{:25} {}\n", "Total Runtime:", format_runtime(pack.total_runtime)));
    if pack.error_bitmask != 0 {
        let errors = jk_bms::error_bitmask_to_strings(pack.error_bitmask);
        out.push_str(&format!("{:25} {:04X} ({})\n", "Errors:", pack.error_bitmask, errors.join(", ")));
    }
    out.push_str(&format!("{:25} {:?}\n", "Protocol:", pack.protocol_version));
    out
}

fn format_runtime(seconds: u32) -> String {
    let days = seconds / 86400;
    let hours = (seconds % 86400) / 3600;
    let mins = (seconds % 3600) / 60;
    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins)
    }
}

fn format_csv(info: &JkInfo, pack: &MybmmPack) -> String {
    let mut out = String::new();
    out.push_str(&format!("Voltage,{:.3}\n", pack.voltage));
    out.push_str(&format!("Current,{:.3}\n", pack.current));
    out.push_str(&format!("Power,{:.3}\n", pack.power));
    out.push_str(&format!("Cells,{}\n", pack.cells));
    for i in 0..pack.cells as usize {
        out.push_str(&format!("Cell{},{:.3}\n", i + 1, pack.cellvolt[i]));
        out.push_str(&format!("Cell{}Resistance,{:.3}\n", i + 1, pack.cellres[i]));
    }
    out.push_str(&format!("CellMin,{:.3}\n", info.cell_min));
    out.push_str(&format!("CellMax,{:.3}\n", info.cell_max));
    out.push_str(&format!("CellDiff,{:.3}\n", info.cell_diff));
    out.push_str(&format!("CellAvg,{:.3}\n", info.cell_avg));
    out.push_str(&format!("MOSFETTemp,{:.1}\n", pack.power_tube_temp));
    out.push_str(&format!("TempProbes,{}\n", pack.ntemps));
    for i in 0..pack.ntemps as usize {
        out.push_str(&format!("Temp{},{:.1}\n", i + 1, pack.temps[i]));
    }
    out.push_str(&format!("SOC,{:.1}\n", pack.soc));
    out.push_str(&format!("SOH,{:.1}\n", pack.soh));
    out.push_str(&format!("CapacityRemaining,{:.3}\n", pack.capacity_remaining));
    out.push_str(&format!("TotalCapacity,{:.3}\n", pack.total_battery_capacity));
    out.push_str(&format!("ChargingCycles,{}\n", pack.charging_cycles));
    out.push_str(&format!("BalancingCurrent,{:.3}\n", pack.balancing_current));
    out.push_str(&format!("Balancing,{}\n", pack.balancing as i32));
    out.push_str(&format!("ChargingMOS,{}\n", pack.charging as i32));
    out.push_str(&format!("DischargingMOS,{}\n", pack.discharging as i32));
    out.push_str(&format!("Heating,{}\n", pack.heating as i32));
    out.push_str(&format!("TotalRuntime,{}\n", pack.total_runtime));
    out.push_str(&format!("ErrorBitmask,{:04X}\n", pack.error_bitmask));
    out.push_str(&format!("Protocol,{:?}\n", pack.protocol_version));
    out
}

fn format_json(info: &JkInfo, pack: &MybmmPack, pretty: bool) -> String {
    // Round f32 values to avoid IEEE 754 noise (e.g. 3.3170002 instead of 3.317)
    let r3 = |v: f32| -> f32 { (v * 1000.0).round() / 1000.0 };
    let r1 = |v: f32| -> f32 { (v * 10.0).round() / 10.0 };

    #[derive(serde::Serialize)]
    struct JsonOut {
        voltage: f32,
        current: f32,
        power: f32,
        charging_power: f32,
        discharging_power: f32,
        cells: i32,
        cell_voltages: Vec<f32>,
        cell_resistances: Vec<f32>,
        cell_min: f32,
        cell_max: f32,
        cell_diff: f32,
        cell_avg: f32,
        mosfet_temp: f32,
        temp_probes: i32,
        temps: Vec<f32>,
        soc: f32,
        soh: f32,
        capacity_remaining: f32,
        total_capacity: f32,
        charging_cycles: u32,
        balancing_current: f32,
        balancing: bool,
        charging_mos: bool,
        discharging_mos: bool,
        precharging: bool,
        heating: bool,
        total_runtime: u32,
        error_bitmask: u16,
        errors: Vec<&'static str>,
        protocol: String,
        model: String,
        hardware_version: String,
        software_version: String,
    }

    let j = JsonOut {
        voltage: r3(pack.voltage),
        current: r3(pack.current),
        power: r3(pack.power),
        charging_power: r3(pack.charging_power),
        discharging_power: r3(pack.discharging_power),
        cells: pack.cells,
        cell_voltages: pack.cellvolt[..pack.cells as usize].iter().map(|&v| r3(v)).collect(),
        cell_resistances: pack.cellres[..pack.cells as usize].iter().map(|&v| r3(v)).collect(),
        cell_min: r3(info.cell_min),
        cell_max: r3(info.cell_max),
        cell_diff: r3(info.cell_diff),
        cell_avg: r3(info.cell_avg),
        mosfet_temp: r1(pack.power_tube_temp),
        temp_probes: pack.ntemps,
        temps: pack.temps[..pack.ntemps as usize].iter().map(|&v| r1(v)).collect(),
        soc: pack.soc,
        soh: pack.soh,
        capacity_remaining: r3(pack.capacity_remaining),
        total_capacity: r3(pack.total_battery_capacity),
        charging_cycles: pack.charging_cycles,
        balancing_current: r3(pack.balancing_current),
        balancing: pack.balancing,
        charging_mos: pack.charging,
        discharging_mos: pack.discharging,
        precharging: pack.precharging,
        heating: pack.heating,
        total_runtime: pack.total_runtime,
        error_bitmask: pack.error_bitmask,
        errors: jk_bms::error_bitmask_to_strings(pack.error_bitmask),
        protocol: format!("{:?}", pack.protocol_version),
        model: pack.model.clone(),
        hardware_version: pack.hwvers.clone(),
        software_version: pack.swvers.clone(),
    };

    if pretty {
        serde_json::to_string_pretty(&j).unwrap_or_else(|_| "{}".to_string())
    } else {
        serde_json::to_string(&j).unwrap_or_else(|_| "{}".to_string())
    }
}

// ===========================================================================
// Settings output formatting
// ===========================================================================

fn format_settings(s: &jk_bms::JkSettings, version: &ProtocolVersion, cli: &Cli) -> String {
    match cli.format {
        OutputFormat::Json => format_settings_json(s, version, cli.pretty),
        OutputFormat::Csv => format_settings_csv(s, version),
        OutputFormat::Text => format_settings_text(s, version),
    }
}

fn format_settings_text(s: &jk_bms::JkSettings, version: &ProtocolVersion) -> String {
    let r3 = |v: f32| -> f32 { (v * 1000.0).round() / 1000.0 };
    let r1 = |v: f32| -> f32 { (v * 10.0).round() / 10.0 };
    let r0 = |v: f32| -> f32 { (v * 1.0).round() / 1.0 };

    let mut out = String::new();
    out.push_str(&format!("{:25} {:.3} V\n", "Smart Sleep Voltage:", r3(s.smart_sleep_voltage)));
    out.push_str(&format!("{:25} {:.3} V\n", "Cell UVP:", r3(s.cell_uvp)));
    out.push_str(&format!("{:25} {:.3} V\n", "Cell UVP Recovery:", r3(s.cell_uvpr)));
    out.push_str(&format!("{:25} {:.3} V\n", "Cell OVP:", r3(s.cell_ovp)));
    out.push_str(&format!("{:25} {:.3} V\n", "Cell OVP Recovery:", r3(s.cell_ovpr)));
    out.push_str(&format!("{:25} {:.3} V\n", "Balance Trigger Voltage:", r3(s.balance_trigger_voltage)));
    out.push_str(&format!("{:25} {:.3} V\n", "SOC 100% Voltage:", r3(s.cell_soc100_voltage)));
    out.push_str(&format!("{:25} {:.3} V\n", "SOC 0% Voltage:", r3(s.cell_soc0_voltage)));
    out.push_str(&format!("{:25} {:.3} V\n", "Request Charge Voltage:", r3(s.cell_request_charge_voltage)));
    out.push_str(&format!("{:25} {:.3} V\n", "Request Float Voltage:", r3(s.cell_request_float_voltage)));
    out.push_str(&format!("{:25} {:.3} V\n", "Power Off Voltage:", r3(s.power_off_voltage)));
    out.push_str(&format!("{:25} {:.3} V\n", "Balance Starting Voltage:", r3(s.balance_starting_voltage)));
    out.push_str(&format!("{:25} {:.3} A\n", "Max Charge Current:", r3(s.max_charge_current)));
    out.push_str(&format!("{:25} {:.3} A\n", "Max Discharge Current:", r3(s.max_discharge_current)));
    out.push_str(&format!("{:25} {:.3} A\n", "Max Balance Current:", r3(s.max_balance_current)));
    out.push_str(&format!("{:25} {:.0} s\n", "Charge OCP Delay:", r0(s.charge_ocp_delay)));
    out.push_str(&format!("{:25} {:.0} s\n", "Charge OCP Recovery:", r0(s.charge_ocp_recovery)));
    out.push_str(&format!("{:25} {:.0} s\n", "Discharge OCP Delay:", r0(s.discharge_ocp_delay)));
    out.push_str(&format!("{:25} {:.0} s\n", "Discharge OCP Recovery:", r0(s.discharge_ocp_recovery)));
    out.push_str(&format!("{:25} {:.0} s\n", "SCP Recovery:", r0(s.scp_recovery)));
    out.push_str(&format!("{:25} {:.0} μs\n", "SCP Delay:", r0(s.scp_delay)));
    out.push_str(&format!("{:25} {:.1} °C\n", "Charge OTP:", r1(s.charge_otp)));
    out.push_str(&format!("{:25} {:.1} °C\n", "Charge OTP Recovery:", r1(s.charge_otp_recovery)));
    out.push_str(&format!("{:25} {:.1} °C\n", "Discharge OTP:", r1(s.discharge_otp)));
    out.push_str(&format!("{:25} {:.1} °C\n", "Discharge OTP Recovery:", r1(s.discharge_otp_recovery)));
    out.push_str(&format!("{:25} {:.1} °C\n", "Charge UTP:", r1(s.charge_utp)));
    out.push_str(&format!("{:25} {:.1} °C\n", "Charge UTP Recovery:", r1(s.charge_utp_recovery)));
    out.push_str(&format!("{:25} {:.1} °C\n", "Power Tube OTP:", r1(s.power_tube_otp)));
    out.push_str(&format!("{:25} {:.1} °C\n", "Power Tube OTP Recovery:", r1(s.power_tube_otp_recovery)));
    out.push_str(&format!("{:25} {}\n", "Cell Count:", s.cell_count));
    out.push_str(&format!("{:25} {:.3} Ah\n", "Battery Capacity:", r3(s.total_battery_capacity)));
    out.push_str(&format!("{:25} {}\n", "Charging Switch:", if s.charging_switch { "On" } else { "Off" }));
    out.push_str(&format!("{:25} {}\n", "Discharging Switch:", if s.discharging_switch { "On" } else { "Off" }));
    out.push_str(&format!("{:25} {}\n", "Balancer Switch:", if s.balancer_switch { "On" } else { "Off" }));

    // Wire resistances
    let max_wire = match *version {
        ProtocolVersion::Jk02_24S => 24,
        _ => 32,
    };
    for i in 0..max_wire {
        if s.wire_resistance[i] != 0.0 {
            out.push_str(&format!("{:25} {:.3} Ω\n", format!("Wire Res {}:", i + 1), r3(s.wire_resistance[i])));
        }
    }

    // JK02_32S only fields
    if *version == ProtocolVersion::Jk02_32S {
        out.push_str(&format!("{:25} {:.1} °C\n", "Discharge UTP:", s.discharge_utp));
        out.push_str(&format!("{:25} {:.1} °C\n", "Discharge UTP Recovery:", s.discharge_utp_recovery));
        out.push_str(&format!("{:25} {:.1} °C\n", "Heating Start Temp:", s.heating_start_temperature));
        out.push_str(&format!("{:25} {:.1} °C\n", "Heating Stop Temp:", s.heating_stop_temperature));
        out.push_str(&format!("{:25} {} s\n", "Precharge Time:", s.discharge_precharge_time));
        out.push_str(&format!("{:25} {}\n", "Heating Switch:", if s.heating_switch { "On" } else { "Off" }));
        out.push_str(&format!("{:25} {}\n", "Disable Temp Sensors:", if s.disable_temp_sensors { "On" } else { "Off" }));
        out.push_str(&format!("{:25} {}\n", "Display Always On:", if s.display_always_on { "On" } else { "Off" }));
        out.push_str(&format!("{:25} {}\n", "Smart Sleep:", if s.smart_sleep_switch { "On" } else { "Off" }));
        out.push_str(&format!("{:25} {}\n", "Disable PCL Module:", if s.disable_pcl_module { "On" } else { "Off" }));
        out.push_str(&format!("{:25} {}\n", "Timed Stored Data:", if s.timed_stored_data { "On" } else { "Off" }));
        out.push_str(&format!("{:25} {}\n", "Charging Float Mode:", if s.charging_float_mode { "On" } else { "Off" }));
    }

    out
}

fn format_settings_csv(s: &jk_bms::JkSettings, version: &ProtocolVersion) -> String {
    let r3 = |v: f32| -> f32 { (v * 1000.0).round() / 1000.0 };
    let r1 = |v: f32| -> f32 { (v * 10.0).round() / 10.0 };

    let mut out = String::new();
    out.push_str(&format!("smart_sleep_voltage,{:.3}\n", r3(s.smart_sleep_voltage)));
    out.push_str(&format!("cell_uvp,{:.3}\n", r3(s.cell_uvp)));
    out.push_str(&format!("cell_uvpr,{:.3}\n", r3(s.cell_uvpr)));
    out.push_str(&format!("cell_ovp,{:.3}\n", r3(s.cell_ovp)));
    out.push_str(&format!("cell_ovpr,{:.3}\n", r3(s.cell_ovpr)));
    out.push_str(&format!("balance_trigger_voltage,{:.3}\n", r3(s.balance_trigger_voltage)));
    out.push_str(&format!("cell_soc100_voltage,{:.3}\n", r3(s.cell_soc100_voltage)));
    out.push_str(&format!("cell_soc0_voltage,{:.3}\n", r3(s.cell_soc0_voltage)));
    out.push_str(&format!("cell_request_charge_voltage,{:.3}\n", r3(s.cell_request_charge_voltage)));
    out.push_str(&format!("cell_request_float_voltage,{:.3}\n", r3(s.cell_request_float_voltage)));
    out.push_str(&format!("power_off_voltage,{:.3}\n", r3(s.power_off_voltage)));
    out.push_str(&format!("balance_starting_voltage,{:.3}\n", r3(s.balance_starting_voltage)));
    out.push_str(&format!("max_charge_current,{:.3}\n", r3(s.max_charge_current)));
    out.push_str(&format!("max_discharge_current,{:.3}\n", r3(s.max_discharge_current)));
    out.push_str(&format!("max_balance_current,{:.3}\n", r3(s.max_balance_current)));
    out.push_str(&format!("charge_ocp_delay,{:.0}\n", s.charge_ocp_delay));
    out.push_str(&format!("charge_ocp_recovery,{:.0}\n", s.charge_ocp_recovery));
    out.push_str(&format!("discharge_ocp_delay,{:.0}\n", s.discharge_ocp_delay));
    out.push_str(&format!("discharge_ocp_recovery,{:.0}\n", s.discharge_ocp_recovery));
    out.push_str(&format!("scp_recovery,{:.0}\n", s.scp_recovery));
    out.push_str(&format!("scp_delay,{:.0}\n", s.scp_delay));
    out.push_str(&format!("charge_otp,{:.1}\n", r1(s.charge_otp)));
    out.push_str(&format!("charge_otp_recovery,{:.1}\n", r1(s.charge_otp_recovery)));
    out.push_str(&format!("discharge_otp,{:.1}\n", r1(s.discharge_otp)));
    out.push_str(&format!("discharge_otp_recovery,{:.1}\n", r1(s.discharge_otp_recovery)));
    out.push_str(&format!("charge_utp,{:.1}\n", r1(s.charge_utp)));
    out.push_str(&format!("charge_utp_recovery,{:.1}\n", r1(s.charge_utp_recovery)));
    out.push_str(&format!("power_tube_otp,{:.1}\n", r1(s.power_tube_otp)));
    out.push_str(&format!("power_tube_otp_recovery,{:.1}\n", r1(s.power_tube_otp_recovery)));
    out.push_str(&format!("cell_count,{}\n", s.cell_count));
    out.push_str(&format!("total_battery_capacity,{:.3}\n", r3(s.total_battery_capacity)));
    out.push_str(&format!("charging,{}\n", s.charging_switch as i32));
    out.push_str(&format!("discharging,{}\n", s.discharging_switch as i32));
    out.push_str(&format!("balancer,{}\n", s.balancer_switch as i32));
    if *version == ProtocolVersion::Jk02_32S {
        out.push_str(&format!("discharge_utp,{:.1}\n", s.discharge_utp));
        out.push_str(&format!("discharge_utp_recovery,{:.1}\n", s.discharge_utp_recovery));
        out.push_str(&format!("heating_start_temperature,{:.1}\n", s.heating_start_temperature));
        out.push_str(&format!("heating_stop_temperature,{:.1}\n", s.heating_stop_temperature));
        out.push_str(&format!("discharge_precharge_time,{}\n", s.discharge_precharge_time));
        out.push_str(&format!("heating,{}\n", s.heating_switch as i32));
        out.push_str(&format!("disable_temperature_sensors,{}\n", s.disable_temp_sensors as i32));
        out.push_str(&format!("display_always_on,{}\n", s.display_always_on as i32));
        out.push_str(&format!("smart_sleep,{}\n", s.smart_sleep_switch as i32));
        out.push_str(&format!("disable_pcl_module,{}\n", s.disable_pcl_module as i32));
        out.push_str(&format!("timed_stored_data,{}\n", s.timed_stored_data as i32));
        out.push_str(&format!("charging_float_mode,{}\n", s.charging_float_mode as i32));
    }
    out
}

fn format_settings_json(s: &jk_bms::JkSettings, version: &ProtocolVersion, pretty: bool) -> String {
    let r3 = |v: f32| -> f32 { (v * 1000.0).round() / 1000.0 };
    let r1 = |v: f32| -> f32 { (v * 10.0).round() / 10.0 };

    let mut obj = serde_json::json!({
        "smart_sleep_voltage": r3(s.smart_sleep_voltage),
        "cell_uvp": r3(s.cell_uvp),
        "cell_uvpr": r3(s.cell_uvpr),
        "cell_ovp": r3(s.cell_ovp),
        "cell_ovpr": r3(s.cell_ovpr),
        "balance_trigger_voltage": r3(s.balance_trigger_voltage),
        "cell_soc100_voltage": r3(s.cell_soc100_voltage),
        "cell_soc0_voltage": r3(s.cell_soc0_voltage),
        "cell_request_charge_voltage": r3(s.cell_request_charge_voltage),
        "cell_request_float_voltage": r3(s.cell_request_float_voltage),
        "power_off_voltage": r3(s.power_off_voltage),
        "balance_starting_voltage": r3(s.balance_starting_voltage),
        "max_charge_current": r3(s.max_charge_current),
        "max_discharge_current": r3(s.max_discharge_current),
        "max_balance_current": r3(s.max_balance_current),
        "charge_ocp_delay": s.charge_ocp_delay,
        "charge_ocp_recovery": s.charge_ocp_recovery,
        "discharge_ocp_delay": s.discharge_ocp_delay,
        "discharge_ocp_recovery": s.discharge_ocp_recovery,
        "scp_recovery": s.scp_recovery,
        "scp_delay": s.scp_delay,
        "charge_otp": r1(s.charge_otp),
        "charge_otp_recovery": r1(s.charge_otp_recovery),
        "discharge_otp": r1(s.discharge_otp),
        "discharge_otp_recovery": r1(s.discharge_otp_recovery),
        "charge_utp": r1(s.charge_utp),
        "charge_utp_recovery": r1(s.charge_utp_recovery),
        "power_tube_otp": r1(s.power_tube_otp),
        "power_tube_otp_recovery": r1(s.power_tube_otp_recovery),
        "cell_count": s.cell_count,
        "total_battery_capacity": r3(s.total_battery_capacity),
        "charging": s.charging_switch,
        "discharging": s.discharging_switch,
        "balancer": s.balancer_switch,
    });

    if *version == ProtocolVersion::Jk02_32S {
        obj.as_object_mut().unwrap().insert(
            "discharge_utp".to_string(), serde_json::json!(s.discharge_utp),
        );
        obj.as_object_mut().unwrap().insert(
            "discharge_utp_recovery".to_string(), serde_json::json!(s.discharge_utp_recovery),
        );
        obj.as_object_mut().unwrap().insert(
            "heating_start_temperature".to_string(), serde_json::json!(s.heating_start_temperature),
        );
        obj.as_object_mut().unwrap().insert(
            "heating_stop_temperature".to_string(), serde_json::json!(s.heating_stop_temperature),
        );
        obj.as_object_mut().unwrap().insert(
            "discharge_precharge_time".to_string(), serde_json::json!(s.discharge_precharge_time),
        );
        obj.as_object_mut().unwrap().insert(
            "heating".to_string(), serde_json::json!(s.heating_switch),
        );
        obj.as_object_mut().unwrap().insert(
            "disable_temperature_sensors".to_string(), serde_json::json!(s.disable_temp_sensors),
        );
        obj.as_object_mut().unwrap().insert(
            "display_always_on".to_string(), serde_json::json!(s.display_always_on),
        );
        obj.as_object_mut().unwrap().insert(
            "smart_sleep".to_string(), serde_json::json!(s.smart_sleep_switch),
        );
        obj.as_object_mut().unwrap().insert(
            "disable_pcl_module".to_string(), serde_json::json!(s.disable_pcl_module),
        );
        obj.as_object_mut().unwrap().insert(
            "timed_stored_data".to_string(), serde_json::json!(s.timed_stored_data),
        );
        obj.as_object_mut().unwrap().insert(
            "charging_float_mode".to_string(), serde_json::json!(s.charging_float_mode),
        );
    }

    if pretty {
        serde_json::to_string_pretty(&obj).unwrap_or_else(|_| "{}".to_string())
    } else {
        serde_json::to_string(&obj).unwrap_or_else(|_| "{}".to_string())
    }
}

