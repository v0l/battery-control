//! `anker` — a small CLI for Anker SOLIX portable power stations over BLE.

use anker_solix::{scan, Device, Result, Telemetry};
use clap::{Parser, Subcommand, ValueEnum};
use std::time::Duration;

mod output;

#[derive(Parser)]
#[command(
    name = "anker",
    version,
    about = "Monitor and control Anker SOLIX power stations over Bluetooth (no app, no cloud)"
)]
struct Cli {
    /// Device name substring or MAC address to target.
    #[arg(short, long, global = true, default_value = "C1000")]
    device: String,

    /// Seconds to scan when locating the device.
    #[arg(long, global = true, default_value_t = 6)]
    scan_secs: u64,

    /// Output format.
    #[arg(short, long, global = true, value_enum, default_value_t = Format::Text)]
    format: Format,

    /// Increase log verbosity (-v, -vv).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Format {
    Text,
    Json,
}

#[derive(Subcommand)]
enum Command {
    /// Scan for nearby SOLIX devices and list them.
    Scan,
    /// Connect and print a single telemetry snapshot.
    Status,
    /// Connect and stream telemetry until interrupted.
    Monitor {
        /// Seconds between refreshes (best-effort; device pushes when it wants).
        #[arg(long, default_value_t = 2)]
        interval: u64,
    },
    /// Control the AC output.
    Ac {
        #[arg(value_enum)]
        state: OnOff,
    },
    /// Control the DC (12 V) output.
    Dc {
        #[arg(value_enum)]
        state: OnOff,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum OnOff {
    On,
    Off,
}

impl OnOff {
    fn as_bool(self) -> bool {
        matches!(self, OnOff::On)
    }
}

fn main() {
    let cli = Cli::parse();
    init_logging(cli.verbose);

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("failed to start async runtime: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = runtime.block_on(run(&cli)) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn init_logging(verbose: u8) {
    let level = match verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level)).init();
}

async fn run(cli: &Cli) -> Result<()> {
    match &cli.command {
        Command::Scan => cmd_scan(cli).await,
        Command::Status => cmd_status(cli).await,
        Command::Monitor { interval } => cmd_monitor(cli, *interval).await,
        Command::Ac { state } => cmd_output(cli, Line::Ac, state.as_bool()).await,
        Command::Dc { state } => cmd_output(cli, Line::Dc, state.as_bool()).await,
    }
}

enum Line {
    Ac,
    Dc,
}

async fn cmd_scan(cli: &Cli) -> Result<()> {
    eprintln!("scanning for {}s...", cli.scan_secs);
    let devices = scan(cli.scan_secs).await?;
    match cli.format {
        Format::Json => println!("{}", output::scan_json(&devices)),
        Format::Text => {
            if devices.is_empty() {
                println!("no SOLIX devices found");
            } else {
                println!("{:<24} {:<38} {:<12} RSSI", "NAME", "ID", "MODEL");
                for d in &devices {
                    let rssi = d.rssi.map(|r| format!("{r} dBm")).unwrap_or_else(|| "-".into());
                    println!(
                        "{:<24} {:<38} {:<12} {}",
                        d.name,
                        d.id,
                        format!("{:?}", d.model),
                        rssi
                    );
                }
            }
        }
    }
    Ok(())
}

async fn cmd_status(cli: &Cli) -> Result<()> {
    let mut dev = connect(cli).await?;
    let t = dev.next_telemetry(Duration::from_secs(12)).await?;
    print_telemetry(cli, dev.name(), &t);
    let _ = dev.disconnect().await;
    Ok(())
}

async fn cmd_monitor(cli: &Cli, interval: u64) -> Result<()> {
    let mut dev = connect(cli).await?;
    eprintln!("streaming telemetry from '{}' (Ctrl-C to stop)", dev.name());
    loop {
        match dev.next_telemetry(Duration::from_secs(15)).await {
            Ok(t) => print_telemetry(cli, dev.name(), &t),
            Err(e) => {
                eprintln!("telemetry error: {e}");
                break;
            }
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
    let _ = dev.disconnect().await;
    Ok(())
}

async fn cmd_output(cli: &Cli, line: Line, on: bool) -> Result<()> {
    let mut dev = connect(cli).await?;
    match line {
        Line::Ac => dev.set_ac(on).await?,
        Line::Dc => dev.set_dc(on).await?,
    }
    let what = match line {
        Line::Ac => "AC",
        Line::Dc => "DC",
    };
    println!("{what} output -> {}", if on { "ON" } else { "OFF" });
    let _ = dev.disconnect().await;
    Ok(())
}

async fn connect(cli: &Cli) -> Result<Device> {
    eprintln!("connecting to '{}'...", cli.device);
    let dev = Device::find_and_connect(&cli.device, cli.scan_secs).await?;
    eprintln!("connected & negotiated ({:?})", dev.model());
    Ok(dev)
}

fn print_telemetry(cli: &Cli, name: &str, t: &Telemetry) {
    match cli.format {
        Format::Json => println!("{}", output::telemetry_json(name, t)),
        Format::Text => print!("{}", output::telemetry_text(name, t)),
    }
}
