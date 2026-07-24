//! `pacetool` — read PACE-BMS packs over RS485 (Modbus).

use clap::{Parser, Subcommand};
use pace_bms::{PaceBms, PaceData, Result};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "pacetool", about = "PACE-BMS PACE_MODBUS tool (RS485)")]
struct Cli {
    /// Serial port, e.g. /dev/ttyUSB0 (optionally `path,baud`)
    #[arg(short = 'p', long, default_value = "/dev/ttyUSB0")]
    port: String,
    /// Baud rate
    #[arg(short, long, default_value_t = 9600)]
    baud: u32,
    /// Pack bus address (1..N)
    #[arg(short, long, default_value_t = 1)]
    address: u8,
    /// Output as JSON
    #[arg(long)]
    json: bool,
    /// Verbose logging (dumps raw Modbus frames)
    #[arg(short, long)]
    debug: bool,
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read live data once (default)
    Read,
    /// Stream live data until Ctrl-C
    Monitor {
        #[arg(long, default_value_t = 2)]
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

    let mut bms = PaceBms::open_serial(&cli.port, cli.baud, cli.address).await?;
    match &cli.cmd {
        Command::Read => {
            let d = bms.read().await?;
            print_data(d, cli.json);
        }
        Command::Monitor { interval } => loop {
            match bms.read().await {
                Ok(d) => print_data(d, cli.json),
                Err(e) => eprintln!("read error: {e}"),
            }
            tokio::time::sleep(Duration::from_secs(*interval)).await;
        },
    }
    let _ = bms.disconnect().await;
    Ok(())
}

fn print_data(d: &PaceData, json: bool) {
    if json {
        let cells: Vec<String> = d.cells.iter().map(|v| format!("{v:.3}")).collect();
        let temps: Vec<String> = d.temps.iter().map(|v| format!("{v:.1}")).collect();
        println!(
            "{{\"soc\":{},\"soh\":{},\"voltage\":{:.2},\"current\":{:.2},\"power\":{:.1},\"remaining_ah\":{:.2},\"full_ah\":{:.2},\"cycles\":{},\"charging\":{},\"discharging\":{},\"cells\":[{}],\"temps\":[{}]}}",
            d.soc, d.soh, d.voltage, d.current, d.power, d.remaining_ah, d.full_ah, d.cycles,
            d.charging, d.discharging, cells.join(","), temps.join(",")
        );
    } else {
        println!("PACE ({} cells)", d.cells.len());
        println!(
            "  SOC {}%  SOH {}%   {:.2} V   {:.2} A   {:.1} W",
            d.soc, d.soh, d.voltage, d.current, d.power
        );
        println!(
            "  {:.1}/{:.1} Ah   {} cycles   charge:{}  discharge:{}",
            d.remaining_ah,
            d.full_ah,
            d.cycles,
            if d.charging { "on" } else { "off" },
            if d.discharging { "on" } else { "off" }
        );
        if !d.cells.is_empty() {
            let min = d.cells.iter().cloned().fold(f32::MAX, f32::min);
            let max = d.cells.iter().cloned().fold(f32::MIN, f32::max);
            println!(
                "  cells: {}  (Δ {:.0} mV)",
                d.cells.iter().map(|v| format!("{v:.3}")).collect::<Vec<_>>().join(" "),
                (max - min) * 1000.0
            );
        }
        let mut temps: Vec<String> = d.temps.iter().map(|t| format!("{t:.1}°C")).collect();
        if let Some(t) = d.mosfet_temp {
            temps.push(format!("MOS {t:.1}°C"));
        }
        if let Some(t) = d.environment_temp {
            temps.push(format!("env {t:.1}°C"));
        }
        if !temps.is_empty() {
            println!("  temps: {}", temps.join("  "));
        }
        let alarms = d.alarms();
        if !alarms.is_empty() {
            println!("  ALARMS: {}", alarms.join(", "));
        }
    }
}
