//! `bluettitool` — discover and control Bluetti power stations over local BLE.

use bluetti::{Bluetti, BluettiData, Result};
use clap::{Parser, Subcommand};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "bluettitool", about = "Bluetti power station tool (local BLE)")]
struct Cli {
    /// BLE peripheral id (from `scan`), optionally `bt:<id>`
    #[arg(short = 't', long, global = true)]
    transport: Option<String>,
    #[arg(long, global = true)]
    json: bool,
    #[arg(short, long, global = true)]
    debug: bool,
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Discover Bluetti devices over BLE
    Scan,
    /// Read live data once (default)
    Read,
    /// Stream live data until Ctrl-C
    Monitor {
        #[arg(long, default_value_t = 3)]
        interval: u64,
    },
    /// Toggle an output: `set ac on` / `set dc off`
    Set { id: String, value: String },
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
            let devices = bluetti::scan(4).await?;
            if devices.is_empty() {
                println!("no Bluetti devices found");
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
            let mut b = connect(&cli).await?;
            let d = b.read().await?;
            print_data("Bluetti", d, cli.json);
            let _ = b.disconnect().await;
            Ok(())
        }
        Command::Monitor { interval } => {
            let mut b = connect(&cli).await?;
            loop {
                match b.read().await {
                    Ok(d) => print_data("Bluetti", d, cli.json),
                    Err(e) => eprintln!("read error: {e}"),
                }
                tokio::time::sleep(Duration::from_secs(*interval)).await;
            }
        }
        Command::Set { id, value } => {
            let on = matches!(value.as_str(), "on" | "true" | "1" | "enable" | "enabled");
            let mut b = connect(&cli).await?;
            b.set(id, on).await?;
            println!("{id} -> {}", if on { "on" } else { "off" });
            let _ = b.disconnect().await;
            Ok(())
        }
    }
}

async fn connect(cli: &Cli) -> Result<Bluetti> {
    let t = cli
        .transport
        .as_deref()
        .ok_or_else(|| bluetti::Error::Transport("need --transport bt:<id>".into()))?;
    let id = t.strip_prefix("bt:").unwrap_or(t);
    Bluetti::connect_ble(id).await
}

fn print_data(model: &str, d: &BluettiData, json: bool) {
    if json {
        let cells: Vec<String> = d.cells.iter().map(|v| format!("{v:.2}")).collect();
        println!(
            "{{\"model\":\"{model}\",\"soc\":{},\"input_power\":{},\"output_power\":{},\"ac_input\":{},\"ac_output\":{},\"dc_input\":{},\"dc_output\":{},\"ac_output_on\":{},\"dc_output_on\":{},\"voltage\":{:.2},\"cells\":[{}]}}",
            d.total_battery_percent, d.input_power(), d.output_power(),
            d.ac_input_power, d.ac_output_power, d.dc_input_power, d.dc_output_power,
            d.ac_output_on, d.dc_output_on, d.total_battery_voltage, cells.join(",")
        );
    } else {
        let name = d.device_type.as_deref().unwrap_or(model);
        println!("{name}");
        println!(
            "  SOC {}%   in {} W   out {} W   {:.2} V",
            d.total_battery_percent, d.input_power(), d.output_power(), d.total_battery_voltage
        );
        println!(
            "  AC: in {} W / out {} W [{}]   DC: in {} W / out {} W [{}]",
            d.ac_input_power,
            d.ac_output_power,
            if d.ac_output_on { "on" } else { "off" },
            d.dc_input_power,
            d.dc_output_power,
            if d.dc_output_on { "on" } else { "off" }
        );
        if !d.cells.is_empty() {
            println!(
                "  cells: {}",
                d.cells.iter().map(|v| format!("{v:.2}")).collect::<Vec<_>>().join(" ")
            );
        }
    }
}
