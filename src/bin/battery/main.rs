//! `battery` — a backend-agnostic CLI for batteries, BMSes and power stations.
//!
//! Batteries are identified by their **hardware id** (BLE address/UUID or serial
//! port), discovered across every transport. You never specify a backend.

use battery_control::{
    discover, resolve, Battery, Command, DiscoverOptions, PortKind, Result,
};
use clap::{Parser, Subcommand, ValueEnum};
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
    /// Control a battery, e.g. `battery set <id> ac on`, `set <id> charge-limit 80`.
    Set {
        query: String,
        #[arg(value_enum)]
        target: Target,
        /// `on`/`off` for switches, or a number for charge-limit.
        value: String,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Target {
    Ac,
    Dc,
    Charge,
    Discharge,
    Balancer,
    ChargeLimit,
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
            let mut bat = connect(&cli, query).await?;
            eprintln!("streaming '{}' (Ctrl-C to stop)", bat.info().id_label());
            loop {
                match bat.status().await {
                    Ok(s) => output::print_status(bat.info(), &s, cli.json),
                    Err(e) => {
                        eprintln!("status error: {e}");
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_secs(*interval)).await;
            }
        }
        Cmd::Set {
            query,
            target,
            value,
        } => {
            let cmd = build_command(*target, value)?;
            let mut bat = connect(&cli, query).await?;
            bat.execute(cmd).await?;
            println!("ok: {} {:?} {}", query, target, value);
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

fn build_command(target: Target, value: &str) -> Result<Command> {
    let on = || -> Result<bool> {
        match value.to_ascii_lowercase().as_str() {
            "on" | "true" | "1" => Ok(true),
            "off" | "false" | "0" => Ok(false),
            _ => Err(battery_control::Error::InvalidArgument(format!(
                "expected on/off, got '{value}'"
            ))),
        }
    };
    Ok(match target {
        Target::Ac => Command::SetPort {
            kind: PortKind::Ac,
            on: on()?,
        },
        Target::Dc => Command::SetPort {
            kind: PortKind::Dc,
            on: on()?,
        },
        Target::Charge => Command::SetCharging(on()?),
        Target::Discharge => Command::SetDischarging(on()?),
        Target::Balancer => Command::SetBalancer(on()?),
        Target::ChargeLimit => {
            let pct: u8 = value
                .parse()
                .map_err(|_| battery_control::Error::InvalidArgument(format!("bad %: {value}")))?;
            Command::SetChargeLimit(pct)
        }
    })
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
