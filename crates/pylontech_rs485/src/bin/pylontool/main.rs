//! `pylontool` — read Pylontech batteries over the RS485 console port.

use clap::{Parser, Subcommand};
use pylontech_rs485::{PylontechData, PylontechRs485, Result};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "pylontool", about = "Pylontech RS485 console tool (US2000/US3000)")]
struct Cli {
    /// Console serial port
    #[arg(short = 'p', long, default_value = "/dev/ttyUSB0")]
    port: String,
    /// Baud rate (US2000/US3000 console is 115200)
    #[arg(short, long, default_value_t = 115200)]
    baud: u32,
    /// Pack address (the console usually answers on 2)
    #[arg(short, long, default_value_t = 2)]
    address: u8,
    #[arg(long)]
    json: bool,
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

    let mut bms = PylontechRs485::open_serial(&cli.port, cli.baud, cli.address).await?;
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

fn print_data(d: &PylontechData, json: bool) {
    if json {
        println!(
            "{{\"modules\":{},\"soc\":{:.1},\"voltage\":{:.3},\"current\":{:.2},\"power\":{:.1},\"remaining_ah\":{:.2},\"total_ah\":{:.2}}}",
            d.modules.len(), d.soc(), d.voltage(), d.current(), d.power(), d.remaining_ah(), d.total_ah()
        );
    } else {
        println!(
            "Pylontech ({} modules)  SOC {:.0}%   {:.2} V   {:.2} A   {:.0} W   {:.1}/{:.1} Ah",
            d.modules.len(),
            d.soc(),
            d.voltage(),
            d.current(),
            d.power(),
            d.remaining_ah(),
            d.total_ah()
        );
        for (i, m) in d.modules.iter().enumerate() {
            let min = m.cells.iter().cloned().fold(f32::MAX, f32::min);
            let max = m.cells.iter().cloned().fold(f32::MIN, f32::max);
            println!(
                "  #{i}: {:.2} V  {:.1} A  {} cells (Δ {:.0} mV)  {} cyc  {:.1}°C",
                m.voltage,
                m.current,
                m.cells.len(),
                (max - min) * 1000.0,
                m.cycles,
                m.temps.first().copied().unwrap_or(0.0)
            );
        }
    }
}
