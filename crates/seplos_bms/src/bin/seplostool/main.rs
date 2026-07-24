//! `seplostool` — read Seplos V3 packs over RS485 (Modbus).

use clap::{Parser, Subcommand};
use seplos_bms::{SeplosBms, SeplosData, Result};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "seplostool", about = "Seplos BMS V3 tool (RS485 Modbus)")]
struct Cli {
    /// Serial port, e.g. /dev/ttyUSB0 (optionally `path,baud`)
    #[arg(short = 'p', long, default_value = "/dev/ttyUSB0")]
    port: String,
    /// Baud rate
    #[arg(short, long, default_value_t = 19200)]
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

    let mut bms = SeplosBms::open_serial(&cli.port, cli.baud, cli.address).await?;
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

fn print_data(d: &SeplosData, json: bool) {
    if json {
        let cells: Vec<String> = d.cells.iter().map(|v| format!("{v:.3}")).collect();
        let temps: Vec<String> = d.cell_temps.iter().map(|v| format!("{v:.1}")).collect();
        println!(
            "{{\"soc\":{:.1},\"soh\":{:.1},\"voltage\":{:.2},\"current\":{:.2},\"power\":{:.1},\"remaining_ah\":{:.2},\"total_ah\":{:.2},\"cycles\":{},\"cells\":[{}],\"cell_temps\":[{}],\"ambient_temp\":{:.1},\"power_temp\":{:.1}}}",
            d.soc, d.soh, d.voltage, d.current, d.power, d.remaining_ah, d.total_ah, d.cycles,
            cells.join(","), temps.join(","), d.ambient_temp, d.power_temp
        );
    } else {
        println!("Seplos V3 ({} cells)", d.cells.len());
        println!(
            "  SOC {:.1}%  SOH {:.1}%   {:.2} V   {:.2} A   {:.1} W",
            d.soc, d.soh, d.voltage, d.current, d.power
        );
        println!("  {:.2}/{:.1} Ah   {} cycles", d.remaining_ah, d.total_ah, d.cycles);
        if !d.cells.is_empty() {
            let min = d.cells.iter().cloned().fold(f32::MAX, f32::min);
            let max = d.cells.iter().cloned().fold(f32::MIN, f32::max);
            println!(
                "  cells: {}  (Δ {:.0} mV)",
                d.cells.iter().map(|v| format!("{v:.3}")).collect::<Vec<_>>().join(" "),
                (max - min) * 1000.0
            );
        }
        let mut temps: Vec<String> = d.cell_temps.iter().map(|t| format!("{t:.1}°C")).collect();
        temps.push(format!("amb {:.1}°C", d.ambient_temp));
        temps.push(format!("pwr {:.1}°C", d.power_temp));
        println!("  temps: {}", temps.join("  "));
    }
}
