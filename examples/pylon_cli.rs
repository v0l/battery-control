use battery_control::backends::PylontechCli;
use battery_control::Battery;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "/dev/ttyUSB2".into());
    let mut bat = PylontechCli::open_serial(&path, 115200).await?;
    let s = bat.status().await?;
    println!("{:?}", bat.info());
    println!("{:#?}", s);
    Ok(())
}
