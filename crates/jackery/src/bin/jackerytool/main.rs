//! `jackerytool` — discover and control Jackery power stations over local BLE.

use clap::{Parser, Subcommand};
use jackery::{Jackery, JackeryData, Result};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "jackerytool", about = "Jackery power station tool (local BLE)")]
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
    /// Discover Jackery devices over BLE (also derives their keys)
    Scan,
    /// Read live data once (default)
    Read,
    /// Stream live data until Ctrl-C
    Monitor {
        #[arg(long, default_value_t = 5)]
        interval: u64,
    },
    /// Toggle an output: `set ac on` / `set dc off` / `set usb on` / `set car off`
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
            let devices = jackery::scan(6).await?;
            if devices.is_empty() {
                println!("no Jackery devices found");
            }
            for d in devices {
                println!(
                    "bt:{}  {}  {}  rssi={}",
                    d.id,
                    d.serial,
                    jackery::model_name(d.model),
                    d.rssi.map(|r| r.to_string()).unwrap_or_else(|| "?".into())
                );
            }
            Ok(())
        }
        Command::Read => {
            let mut j = connect(&cli).await?;
            let model = j.model();
            let d = j.read().await?;
            print_data(&model, d, cli.json);
            let _ = j.disconnect().await;
            Ok(())
        }
        Command::Monitor { interval } => {
            let mut j = connect(&cli).await?;
            let model = j.model();
            loop {
                match j.read().await {
                    Ok(d) => print_data(&model, d, cli.json),
                    Err(e) => eprintln!("read error: {e}"),
                }
                tokio::time::sleep(Duration::from_secs(*interval)).await;
            }
        }
        Command::Set { id, value } => {
            let on = matches!(value.as_str(), "on" | "true" | "1" | "enable" | "enabled");
            let mut j = connect(&cli).await?;
            j.set(id, on).await?;
            println!("{id} -> {}", if on { "on" } else { "off" });
            let _ = j.disconnect().await;
            Ok(())
        }
    }
}

/// Connect requires the advertisement-derived key, so a scan is done first to
/// find the device by id and recover its key.
async fn connect(cli: &Cli) -> Result<Jackery> {
    let t = cli
        .transport
        .as_deref()
        .ok_or_else(|| jackery::Error::Transport("need --transport bt:<id>".into()))?;
    let id = t.strip_prefix("bt:").unwrap_or(t);
    let dev = jackery::scan(6)
        .await?
        .into_iter()
        .find(|d| d.id.eq_ignore_ascii_case(id))
        .ok_or(jackery::Error::NotFound)?;
    Jackery::connect_ble(&dev.id, dev.key, dev.model, dev.serial).await
}

fn print_data(model: &str, d: &JackeryData, json: bool) {
    if json {
        println!(
            "{{\"model\":\"{model}\",\"soc\":{},\"input_power\":{},\"output_power\":{},\"temperature\":{:.1},\"ac_on\":{},\"dc_on\":{},\"error\":{}}}",
            d.rb, d.ip, d.op, d.temperature_c(), d.ac_on(), d.dc_on(), d.ec
        );
    } else {
        println!("{model}");
        println!(
            "  SOC {}%   in {} W   out {} W   {:.1}°C",
            d.rb, d.ip, d.op, d.temperature_c()
        );
        println!(
            "  AC:{}  DC:{}  USB:{}  car:{}",
            on(d.ac_on()), on(d.dc_on()), on(d.usb_on()), on(d.car_on())
        );
        let alarms = d.alarms();
        if !alarms.is_empty() {
            println!("  ALARMS: {}", alarms.join(", "));
        }
    }
}

fn on(v: bool) -> &'static str {
    if v {
        "on"
    } else {
        "off"
    }
}
