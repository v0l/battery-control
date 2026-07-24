//! `soktool` — discover and read SOK Bluetooth batteries.

use clap::{Parser, Subcommand};
use sok_bms::{Result, SokBms, SokData};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "soktool", about = "SOK BLE battery tool")]
struct Cli {
    /// BLE peripheral id (from `scan`)
    #[arg(short = 't', long, global = true)]
    transport: Option<String>,
    /// Output as JSON
    #[arg(long, global = true)]
    json: bool,
    /// Verbose logging
    #[arg(short, long, global = true)]
    debug: bool,
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Discover SOK devices over BLE
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
            let devices = sok_bms::scan(4).await?;
            if devices.is_empty() {
                println!("no SOK devices found");
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

async fn connect(cli: &Cli) -> Result<SokBms> {
    let t = cli
        .transport
        .as_deref()
        .ok_or_else(|| sok_bms::Error::Transport("need --transport bt:<id>".into()))?;
    let id = t.strip_prefix("bt:").unwrap_or(t);
    SokBms::connect_ble(id).await
}

fn print_data(d: &SokData, json: bool) {
    if json {
        let cells: Vec<String> = d.cells.iter().map(|v| format!("{v:.3}")).collect();
        println!(
            "{{\"soc\":{},\"voltage\":{:.2},\"current\":{:.2},\"power\":{:.1},\"capacity\":{:.1},\"cycles\":{},\"temperature\":{:.1},\"cells\":[{}]}}",
            d.soc, d.voltage, d.current, d.power, d.capacity, d.cycles, d.temperature, cells.join(",")
        );
    } else {
        println!("SOK");
        println!(
            "  SOC {}%   {:.2} V   {:.2} A   {:.1} W",
            d.soc, d.voltage, d.current, d.power
        );
        println!(
            "  {:.1} Ah   {} cycles   {:.1}°C",
            d.capacity, d.cycles, d.temperature
        );
        if !d.cells.is_empty() {
            let min = d.cells.iter().cloned().fold(f32::MAX, f32::min);
            let max = d.cells.iter().cloned().fold(f32::MIN, f32::max);
            println!(
                "  cells: {}  (Δ {:.0} mV)",
                d.cells
                    .iter()
                    .map(|v| format!("{v:.3}"))
                    .collect::<Vec<_>>()
                    .join(" "),
                (max - min) * 1000.0
            );
        }
    }
}
