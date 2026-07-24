//! `renogytool` — discover and read Renogy smart batteries over BLE.

use clap::{Parser, Subcommand};
use renogy_bms::{RenogyBms, RenogyData, Result};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "renogytool", about = "Renogy smart battery tool (BT-1/BT-2)")]
struct Cli {
    /// BLE peripheral id (from `scan`), optionally `bt:<id>`
    #[arg(short = 't', long, global = true)]
    transport: Option<String>,
    /// Modbus unit id (hub/daisy-chain batteries use 48/49/50)
    #[arg(long, default_value_t = 0xFF, global = true)]
    unit: u8,
    /// Output as JSON
    #[arg(long, global = true)]
    json: bool,
    /// Verbose logging (dumps raw Modbus frames)
    #[arg(short, long, global = true)]
    debug: bool,
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Discover Renogy devices over BLE
    Scan,
    /// Read live data once (default)
    Read,
    /// Stream live data until Ctrl-C
    Monitor {
        #[arg(long, default_value_t = 5)]
        interval: u64,
    },
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut b = env_logger::Builder::from_default_env();
    b.filter_level(if cli.debug {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    });
    let _ = b.try_init();

    match &cli.cmd {
        Command::Scan => {
            let devices = renogy_bms::scan(4).await?;
            if devices.is_empty() {
                println!("no Renogy devices found");
            }
            for d in devices {
                println!(
                    "bt:{}  {}  rssi={}",
                    d.id,
                    d.name.as_deref().unwrap_or("(unnamed)"),
                    d.rssi.map(|r| r.to_string()).unwrap_or_else(|| "?".into())
                );
            }
            Ok(())
        }
        Command::Read => {
            let mut bms = connect(&cli).await?;
            let d = bms.read().await?;
            print_data(d, cli.json);
            let _ = bms.disconnect().await;
            Ok(())
        }
        Command::Monitor { interval } => {
            let mut bms = connect(&cli).await?;
            loop {
                match bms.read().await {
                    Ok(d) => print_data(d, cli.json),
                    Err(e) => eprintln!("read error: {e}"),
                }
                tokio::time::sleep(Duration::from_secs(*interval)).await;
            }
        }
    }
}

async fn connect(cli: &Cli) -> Result<RenogyBms> {
    let t = cli
        .transport
        .as_deref()
        .ok_or_else(|| renogy_bms::Error::Transport("need --transport bt:<id>".into()))?;
    let id = t.strip_prefix("bt:").unwrap_or(t);
    RenogyBms::connect_ble_as(id, cli.unit).await
}

fn print_data(d: &RenogyData, json: bool) {
    let model = d.model.as_deref().unwrap_or("Renogy");
    if json {
        let cells: Vec<String> = d.cells.iter().map(|v| format!("{v:.2}")).collect();
        let temps: Vec<String> = d.temps.iter().map(|v| format!("{v:.1}")).collect();
        println!(
            "{{\"model\":\"{model}\",\"soc\":{:.1},\"voltage\":{:.2},\"current\":{:.2},\"power\":{:.1},\"remaining_ah\":{:.3},\"capacity_ah\":{:.1},\"cells\":[{}],\"temps\":[{}]}}",
            d.soc, d.voltage, d.current, d.power, d.remaining_ah, d.capacity_ah, cells.join(","), temps.join(",")
        );
    } else {
        println!("{model}");
        println!(
            "  SOC {:.0}%   {:.2} V   {:.2} A   {:.1} W",
            d.soc, d.voltage, d.current, d.power
        );
        println!("  {:.2}/{:.1} Ah", d.remaining_ah, d.capacity_ah);
        if !d.cells.is_empty() {
            let min = d.cells.iter().cloned().fold(f32::MAX, f32::min);
            let max = d.cells.iter().cloned().fold(f32::MIN, f32::max);
            println!(
                "  cells: {}  (Δ {:.0} mV)",
                d.cells.iter().map(|v| format!("{v:.2}")).collect::<Vec<_>>().join(" "),
                (max - min) * 1000.0
            );
        }
        if !d.temps.is_empty() {
            println!(
                "  temps: {}",
                d.temps.iter().map(|t| format!("{t:.1}°C")).collect::<Vec<_>>().join("  ")
            );
        }
    }
}
