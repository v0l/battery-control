//! `jbdtool` — discover and talk to JBD / Xiaoxiang / Overkill BMSes.

use clap::{Parser, Subcommand};
use jbd_bms::{JbdBms, JbdData, Result};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "jbdtool", about = "JBD / Xiaoxiang / Overkill BMS tool")]
struct Cli {
    /// Transport: `bt:<id>` or `serial:<path>[,<baud>]`
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
    /// Discover devices over BLE
    Scan,
    /// Read live data once (default)
    Read,
    /// Stream live data until Ctrl-C
    Monitor {
        /// Seconds between reads
        #[arg(long, default_value_t = 2)]
        interval: u64,
    },
    /// Control a FET: `set charge off` / `set discharge on`
    Set { id: String, value: String },
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut builder = env_logger::Builder::from_default_env();
    builder.filter_level(if cli.debug {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    });
    let _ = builder.try_init();

    match &cli.cmd {
        Command::Scan => scan().await,
        Command::Read => {
            let mut bms = connect(&cli).await?;
            let data = bms.read().await?;
            print_data(data, cli.json);
            let _ = bms.disconnect().await;
            Ok(())
        }
        Command::Monitor { interval } => {
            let mut bms = connect(&cli).await?;
            loop {
                match bms.read().await {
                    Ok(data) => print_data(data, cli.json),
                    Err(e) => eprintln!("read error: {e}"),
                }
                tokio::time::sleep(Duration::from_secs(*interval)).await;
            }
        }
        Command::Set { id, value } => {
            let on = matches!(value.as_str(), "on" | "true" | "1" | "enable" | "enabled");
            let mut bms = connect(&cli).await?;
            bms.read().await?; // learn the other FET's state first
            bms.set(id, on).await?;
            println!("{id} -> {}", if on { "on" } else { "off" });
            let _ = bms.disconnect().await;
            Ok(())
        }
    }
}

async fn scan() -> Result<()> {
    let devices = jbd_bms::scan(4).await?;
    if devices.is_empty() {
        println!("no JBD devices found");
    }
    for d in devices {
        println!(
            "{}  {}  rssi={}",
            d.id,
            d.name.as_deref().unwrap_or("(unnamed)"),
            d.rssi.map(|r| r.to_string()).unwrap_or_else(|| "?".into())
        );
    }
    Ok(())
}

async fn connect(cli: &Cli) -> Result<JbdBms> {
    let t = cli
        .transport
        .as_deref()
        .ok_or_else(|| jbd_bms::Error::Transport("need --transport bt:<id> or serial:<path>".into()))?;
    if let Some(id) = t.strip_prefix("bt:") {
        JbdBms::connect_ble(id).await
    } else if let Some(path) = t.strip_prefix("serial:") {
        JbdBms::connect_serial(path).await
    } else {
        Err(jbd_bms::Error::Transport(format!(
            "unknown transport '{t}' (use bt:<id> or serial:<path>)"
        )))
    }
}

fn print_data(d: &JbdData, json: bool) {
    let b = &d.basic;
    let model = if b.cell_count > 0 {
        format!("JBD {}S", b.cell_count)
    } else {
        "JBD".to_string()
    };
    if json {
        let cells: Vec<String> = d.cells.iter().map(|v| format!("{v:.3}")).collect();
        println!(
            "{{\"model\":\"{model}\",\"soc\":{},\"voltage\":{:.2},\"current\":{:.2},\"power\":{:.1},\"remaining_ah\":{:.2},\"full_ah\":{:.2},\"cycles\":{},\"charging\":{},\"discharging\":{},\"temps\":{:?},\"cells\":[{}],\"alarms\":{:?}}}",
            b.soc, b.voltage, b.current, b.power, b.remaining_ah, b.full_ah, b.cycles,
            b.charging, b.discharging, b.temps, cells.join(","), b.alarms()
        );
    } else {
        println!("{model}");
        println!(
            "  SOC {}%   {:.2} V   {:.2} A   {:.1} W",
            b.soc, b.voltage, b.current, b.power
        );
        println!(
            "  {:.1}/{:.1} Ah   {} cycles   charge:{}  discharge:{}",
            b.remaining_ah,
            b.full_ah,
            b.cycles,
            if b.charging { "on" } else { "off" },
            if b.discharging { "on" } else { "off" }
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
        if !b.temps.is_empty() {
            println!(
                "  temps: {}",
                b.temps
                    .iter()
                    .map(|t| format!("{t:.1}°C"))
                    .collect::<Vec<_>>()
                    .join("  ")
            );
        }
        let alarms = b.alarms();
        if !alarms.is_empty() {
            println!("  ALARMS: {}", alarms.join(", "));
        }
    }
}
