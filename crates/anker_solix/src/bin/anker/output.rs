//! Output formatting for the `anker` CLI.

use anker_solix::{Discovered, Port, PortStatus, Telemetry};

pub fn scan_json(devices: &[Discovered]) -> String {
    let items: Vec<_> = devices
        .iter()
        .map(|d| {
            serde_json::json!({
                "name": d.name,
                "id": d.id,
                "address": d.address,
                "model": format!("{:?}", d.model),
                "rssi": d.rssi,
            })
        })
        .collect();
    serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".into())
}

pub fn telemetry_json(name: &str, t: &Telemetry) -> String {
    let mut v = serde_json::to_value(t).unwrap_or(serde_json::Value::Null);
    if let Some(obj) = v.as_object_mut() {
        obj.insert("device".into(), serde_json::json!(name));
    }
    serde_json::to_string(&v).unwrap_or_else(|_| "{}".into())
}

fn port_str(p: &Port) -> String {
    let status = match p.status {
        PortStatus::Off => "off",
        PortStatus::Output => "on",
        PortStatus::Input => "in",
        PortStatus::Unknown => "-",
    };
    match p.watts {
        Some(w) => format!("{status} ({w} W)"),
        None => status.to_string(),
    }
}

fn opt<T: std::fmt::Display>(v: &Option<T>, unit: &str) -> String {
    match v {
        Some(x) => format!("{x}{unit}"),
        None => "-".to_string(),
    }
}

pub fn telemetry_text(name: &str, t: &Telemetry) -> String {
    let mut out = String::new();
    let row = |out: &mut String, k: &str, v: String| {
        out.push_str(&format!("{k:<22} {v}\n"));
    };

    out.push_str(&format!("── {name} ──\n"));
    row(&mut out, "Battery:", opt(&t.battery_percentage, "%"));
    if t.battery_health.is_some() {
        row(&mut out, "Battery health:", opt(&t.battery_health, "%"));
    }
    if t.battery_percentage_expansion.is_some() {
        row(&mut out, "Expansion battery:", opt(&t.battery_percentage_expansion, "%"));
    }
    if t.min_battery_percentage.is_some() || t.max_battery_percentage.is_some() {
        row(
            &mut out,
            "Charge limits:",
            format!(
                "{}–{}",
                opt(&t.min_battery_percentage, "%"),
                opt(&t.max_battery_percentage, "%")
            ),
        );
    }
    row(&mut out, "Power in:", opt(&t.power_in, " W"));
    row(&mut out, "Power out:", opt(&t.power_out, " W"));
    row(&mut out, "AC:", port_str(&t.ac));
    if t.ac_power_in.is_some() {
        row(&mut out, "AC power in:", opt(&t.ac_power_in, " W"));
    }
    row(&mut out, "DC (12V):", port_str(&t.dc));
    row(&mut out, "Solar:", port_str(&t.solar));
    row(&mut out, "USB-C1:", port_str(&t.usb_c1));
    row(&mut out, "USB-C2:", port_str(&t.usb_c2));
    if t.usb_c3.watts.is_some() || t.usb_c3.status != PortStatus::Unknown {
        row(&mut out, "USB-C3:", port_str(&t.usb_c3));
    }
    row(&mut out, "USB-A1:", port_str(&t.usb_a1));
    if t.usb_a2.watts.is_some() || t.usb_a2.status != PortStatus::Unknown {
        row(&mut out, "USB-A2:", port_str(&t.usb_a2));
    }
    if let Some(h) = t.time_remaining_hours {
        row(&mut out, "Time remaining:", format!("{h:.1} h"));
    }
    row(&mut out, "Temperature:", opt(&t.temperature_c, " °C"));
    if let Some(sn) = &t.serial_number {
        if !sn.is_empty() {
            row(&mut out, "Serial:", sn.clone());
        }
    }
    if let Some(pn) = &t.part_number {
        if !pn.is_empty() {
            row(&mut out, "Part number:", pn.clone());
        }
    }
    if let Some(sw) = &t.software_version {
        row(&mut out, "Firmware:", sw.clone());
    }
    out
}
