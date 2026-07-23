//! `battery` — a backend-agnostic CLI for batteries, BMSes and power stations.
//!
//! Batteries are identified by their **hardware id** (BLE address/UUID or serial
//! port), discovered across every transport. You never specify a backend.

use battery_control::{discover, resolve, Battery, Command, DiscoverOptions, Result};
use clap::{Parser, Subcommand};
use std::time::Duration;

mod output;

#[derive(Parser)]
#[command(
    name = "battery",
    version,
    about = "Monitor and control batteries across many BMS/station backends — no backend flag, ever"
)]
struct Cli {
    /// Seconds to scan BLE.
    #[arg(long, global = true, default_value_t = 6)]
    ble_secs: u64,

    /// Skip probing serial ports.
    #[arg(long, global = true, default_value_t = false)]
    no_serial: bool,

    /// Output as JSON.
    #[arg(long, global = true, default_value_t = false)]
    json: bool,

    /// -v info, -vv debug.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Discover batteries on all transports and list them by hardware id.
    Scan,
    /// Show a battery's status. QUERY is a hardware id (or unique prefix/name).
    Status { query: String },
    /// Stream a battery's status until interrupted.
    Monitor {
        query: String,
        #[arg(long, default_value_t = 2)]
        interval: u64,
    },
    /// Control a battery.
    ///
    /// TARGET is a port/switch id (e.g. `ac`, `dc`, `usb_c1`, `charging`,
    /// `heater`, `display`) or a named setting (e.g. `charge_limit`, `light`).
    /// If VALUE is on/off it's a toggle, otherwise it's a set. Examples:
    /// `battery set <id> ac on`, `battery set <id> charge_limit 80`,
    /// `battery set <id> light high`.
    Set {
        query: String,
        target: String,
        value: String,
    },
}

fn main() {
    let cli = Cli::parse();
    let level = match cli.verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level)).init();

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    if let Err(e) = rt.block_on(run(cli)) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn opts(cli: &Cli) -> DiscoverOptions {
    DiscoverOptions {
        ble_secs: cli.ble_secs,
        probe_serial: !cli.no_serial,
        ..Default::default()
    }
}

async fn run(cli: Cli) -> Result<()> {
    match &cli.command {
        Cmd::Scan => {
            eprintln!("discovering batteries...");
            let devices = discover(&opts(&cli)).await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&devices).unwrap());
            } else {
                output::print_scan(&devices);
            }
        }
        Cmd::Status { query } => {
            let mut bat = connect(&cli, query).await?;
            let s = bat.status().await?;
            output::print_status(bat.info(), &s, cli.json);
        }
        Cmd::Monitor { query, interval } => {
            use futures_util::StreamExt;
            let mut bat = connect(&cli, query).await?;
            eprintln!("streaming '{}' (Ctrl-C to stop)", bat.info().id_label());
            // Unified stream: native push when the backend has one, else
            // poll-and-diff. Fold updates into a running snapshot and reprint
            // at most once per `interval`.
            let interval = Duration::from_secs(*interval);
            let info = bat.info().clone();
            let mut live = battery_control::BatteryStatus::default();
            let mut last_print: Option<std::time::Instant> = None;
            let mut dirty = false;
            let mut updates = bat.updates(interval);
            loop {
                let next = tokio::time::timeout(interval, updates.next()).await;
                match next {
                    Ok(Some(Ok(u))) => {
                        live.apply(&u);
                        dirty = true;
                    }
                    Ok(Some(Err(e))) => {
                        eprintln!("status error: {e}");
                        break;
                    }
                    Ok(None) => break,
                    Err(_elapsed) => {} // no update this tick; maybe flush below
                }
                let due = last_print.is_none_or(|t| t.elapsed() >= interval);
                if dirty && due {
                    output::print_status(&info, &live, cli.json);
                    last_print = Some(std::time::Instant::now());
                    dirty = false;
                }
            }
        }
        Cmd::Set {
            query,
            target,
            value,
        } => {
            let cmd = build_command(target, value)?;
            let mut bat = connect(&cli, query).await?;
            bat.execute(cmd).await?;
            println!("ok: {query} {target} {value}");
        }
    }
    Ok(())
}

async fn connect(cli: &Cli, query: &str) -> Result<Box<dyn Battery>> {
    eprintln!("discovering batteries...");
    let devices = discover(&opts(cli)).await?;
    let chosen = resolve(&devices, query)?;
    eprintln!(
        "connecting to {} [{}] ({})...",
        chosen.label, chosen.id, chosen.backend
    );
    chosen.connect(cli.ble_secs).await
}

/// A boolean value maps to `Toggle`; anything else maps to `Set`.
fn build_command(target: &str, value: &str) -> Result<Command> {
    // Normalize ids: lowercase, hyphens to underscores (charge-limit -> charge_limit).
    let id = target.to_ascii_lowercase().replace('-', "_");
    match value.to_ascii_lowercase().as_str() {
        "on" | "true" | "1" => Ok(Command::Toggle { id, on: true }),
        "off" | "false" | "0" => Ok(Command::Toggle { id, on: false }),
        _ => Ok(Command::Set {
            id,
            value: value.to_string(),
        }),
    }
}

// Small convenience for a display label on DeviceInfo.
trait InfoLabel {
    fn id_label(&self) -> String;
}
impl InfoLabel for battery_control::DeviceInfo {
    fn id_label(&self) -> String {
        self.model.clone().unwrap_or_else(|| self.backend.to_string())
    }
}
