//! `ecoflowtool` — discover and monitor encrypted EcoFlow devices over local BLE.
//!
//! Auth needs your account `user_id` (one-time): log in at the EcoFlow web app
//! and copy the `user_id` from the `/auth/login` response (see ef-ble-reverse).

use clap::{Parser, Subcommand};
use ecoflow::{Ecoflow, Result};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "ecoflowtool", about = "EcoFlow power station tool (local BLE, encrypted)")]
struct Cli {
    /// BLE peripheral id (from `scan`), optionally `bt:<id>`
    #[arg(short = 't', long, global = true)]
    transport: Option<String>,
    /// Device serial (from `scan`)
    #[arg(short, long, global = true)]
    serial: Option<String>,
    /// Account user_id (required; or set ECOFLOW_USER_ID)
    #[arg(short, long, global = true)]
    user_id: Option<String>,
    #[arg(long, global = true)]
    json: bool,
    #[arg(short, long, global = true)]
    debug: bool,
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Discover EcoFlow devices over BLE
    Scan,
    /// Read SOC once (default)
    Read,
    /// Stream telemetry until Ctrl-C
    Monitor {
        #[arg(long, default_value_t = 5)]
        interval: u64,
    },
    /// Dump raw decoded inner packets (cmd_set/cmd_id + payload hex) for mapping
    Packets,
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
            let devices = ecoflow::scan(6).await?;
            if devices.is_empty() {
                println!("no EcoFlow (HD31/Y711) devices found");
            }
            for d in devices {
                println!(
                    "bt:{}  {}  {}  rssi={}",
                    d.id,
                    d.serial,
                    ecoflow::model_name(&d.serial),
                    d.rssi.map(|r| r.to_string()).unwrap_or_else(|| "?".into())
                );
            }
            Ok(())
        }
        Command::Read => {
            let mut dev = connect(&cli).await?;
            let model = dev.model();
            let t = dev.read().await?;
            match t.soc {
                Some(soc) if cli.json => println!("{{\"model\":\"{model}\",\"soc\":{soc}}}"),
                Some(soc) => println!("{model}\n  SOC {soc:.1}%"),
                None => println!("{model}\n  (no SOC decoded yet — try `packets` to inspect)"),
            }
            let _ = dev.disconnect().await;
            Ok(())
        }
        Command::Monitor { interval } => {
            let mut dev = connect(&cli).await?;
            let model = dev.model().to_string();
            loop {
                match dev.read().await {
                    Ok(t) => match t.soc {
                        Some(soc) => println!("{model}: SOC {soc:.1}%"),
                        None => println!("{model}: waiting for telemetry…"),
                    },
                    Err(e) => eprintln!("read error: {e}"),
                }
                tokio::time::sleep(Duration::from_secs(*interval)).await;
            }
        }
        Command::Packets => {
            let mut dev = connect(&cli).await?;
            println!("authenticated to {} — streaming packets (Ctrl-C to stop)", dev.model());
            loop {
                let packets = dev.read_packets().await?;
                for p in packets {
                    let hex: String = p.payload.iter().map(|b| format!("{b:02x}")).collect();
                    println!(
                        "src=0x{:02X} cmd_set=0x{:02X} cmd_id=0x{:02X} len={} {}",
                        p.src,
                        p.cmd_set,
                        p.cmd_id,
                        p.payload.len(),
                        hex
                    );
                }
            }
        }
    }
}

async fn connect(cli: &Cli) -> Result<Ecoflow> {
    let t = cli
        .transport
        .as_deref()
        .ok_or_else(|| ecoflow::Error::Transport("need --transport bt:<id>".into()))?;
    let id = t.strip_prefix("bt:").unwrap_or(t);
    let serial = cli
        .serial
        .as_deref()
        .ok_or_else(|| ecoflow::Error::Transport("need --serial <SN> (from scan)".into()))?;
    let user_id = cli
        .user_id
        .clone()
        .or_else(|| std::env::var("ECOFLOW_USER_ID").ok())
        .ok_or_else(|| ecoflow::Error::Transport("need --user-id (or ECOFLOW_USER_ID)".into()))?;
    Ecoflow::connect_ble(id, serial, &user_id).await
}
