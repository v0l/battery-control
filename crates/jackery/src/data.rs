//! Decoded telemetry — the JSON body of a device-property response.

use serde::Deserialize;

/// Jackery portable device status. Fields are the raw JSON keys; missing keys
/// default to 0. See `porcupin26/private_jack` for the full field list.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct JackeryData {
    pub rb: i64,    // battery %
    pub bt: i64,    // battery temperature ×10 °C
    pub ip: i64,    // total input power (W)
    pub op: i64,    // total output power (W)
    pub acip: i64,  // AC input power (W)
    pub cip: i64,   // DC/solar input power (W)
    pub acps: i64,  // AC output power (W)
    pub acov: i64,  // AC output voltage
    pub acohz: i64, // AC output frequency
    pub oac: i64,   // AC output on
    pub odc: i64,   // DC output on
    pub odcu: i64,  // USB output on (split-DC models)
    pub odcc: i64,  // car output on (split-DC models)
    pub ups: i64,   // UPS mode
    pub sfc: i64,   // super charge
    pub ec: i64,    // error code
    pub en: i64,    // energy-saving timer (min)
}

impl JackeryData {
    pub fn soc(&self) -> f64 {
        self.rb as f64
    }
    pub fn temperature_c(&self) -> f64 {
        self.bt as f64 / 10.0
    }
    pub fn input_power(&self) -> f64 {
        self.ip as f64
    }
    pub fn output_power(&self) -> f64 {
        self.op as f64
    }
    pub fn ac_on(&self) -> bool {
        self.oac == 1
    }
    pub fn dc_on(&self) -> bool {
        self.odc == 1
    }
    pub fn usb_on(&self) -> bool {
        self.odcu == 1
    }
    pub fn car_on(&self) -> bool {
        self.odcc == 1
    }
    pub fn alarms(&self) -> Vec<String> {
        if self.ec != 0 {
            vec![format!("error code {}", self.ec)]
        } else {
            Vec::new()
        }
    }
}

/// Extract and parse the JSON object embedded in a decrypted response payload.
pub fn parse(payload: &[u8]) -> Option<JackeryData> {
    let start = payload.iter().position(|&b| b == b'{')?;
    let end = payload.iter().rposition(|&b| b == b'}')?;
    if end < start {
        return None;
    }
    serde_json::from_slice(&payload[start..=end]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_json() {
        // payload = 00 FC 03 <len> { json }
        let mut payload = vec![0x00, 0xFC, 0x03, 0x00];
        payload.extend_from_slice(br#"{"rb":83,"ip":0,"op":350,"bt":215,"oac":1,"odc":0,"ec":0}"#);
        let d = parse(&payload).expect("parse");
        assert_eq!(d.rb, 83);
        assert!((d.soc() - 83.0).abs() < 1e-9);
        assert!((d.temperature_c() - 21.5).abs() < 1e-9);
        assert_eq!(d.output_power() as i64, 350);
        assert!(d.ac_on() && !d.dc_on());
        assert!(d.alarms().is_empty());
    }
}
