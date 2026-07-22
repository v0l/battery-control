//! Text/JSON rendering for the `battery` CLI.

use battery_control::{BatteryStatus, DeviceInfo, Discovered, PortDirection};

pub fn print_scan(devices: &[Discovered]) {
    if devices.is_empty() {
        println!("no batteries found");
        return;
    }
    println!("{:<44} {:<14} {:<9} LABEL", "HARDWARE ID", "TYPE", "BACKEND");
    for d in devices {
        println!(
            "{:<44} {:<14} {:<9} {}",
            d.id,
            format!("{:?}", d.class),
            d.backend,
            d.label
        );
    }
}

pub fn print_status(info: &DeviceInfo, s: &BatteryStatus, json: bool) {
    if json {
        let mut v = serde_json::to_value(s).unwrap_or(serde_json::Value::Null);
        if let Some(o) = v.as_object_mut() {
            o.insert("backend".into(), serde_json::json!(info.backend));
            o.insert("model".into(), serde_json::json!(info.model));
            o.insert("serial".into(), serde_json::json!(info.serial));
        }
        println!("{}", serde_json::to_string(&v).unwrap());
        return;
    }
    print!("{}", render_text(info, s));
}

fn opt<T: std::fmt::Display>(v: &Option<T>, unit: &str) -> String {
    v.as_ref().map(|x| format!("{x}{unit}")).unwrap_or_else(|| "-".into())
}

fn render_text(info: &DeviceInfo, s: &BatteryStatus) -> String {
    let title = info.model.clone().unwrap_or_else(|| info.backend.to_string());
    let mut o = format!("── {title} ({}) ──\n", info.backend);
    let row = |o: &mut String, k: &str, v: String| o.push_str(&format!("{k:<20} {v}\n"));

    row(&mut o, "SOC:", opt(&s.soc, " %"));
    if s.soh.is_some() {
        row(&mut o, "SOH:", opt(&s.soh, " %"));
    }
    row(&mut o, "Voltage:", opt(&s.voltage, " V"));
    row(&mut o, "Current:", opt(&s.current, " A"));
    if s.power_in.is_some() {
        row(&mut o, "Power in:", opt(&s.power_in, " W"));
    }
    if s.power_out.is_some() {
        row(&mut o, "Power out:", opt(&s.power_out, " W"));
    }
    // Temperatures: one row if a single probe, else one row per named sensor.
    match s.temperatures.as_slice() {
        [] => {}
        [one] => row(&mut o, "Temperature:", format!("{} °C", one.celsius)),
        many => {
            for sensor in many {
                let name = sensor.label.as_deref().unwrap_or(&sensor.id);
                row(&mut o, &format!("Temp {name}:"), format!("{} °C", sensor.celsius));
            }
        }
    }
    if s.charging.is_some() || s.discharging.is_some() {
        row(
            &mut o,
            "MOSFETs:",
            format!("chg {} / dis {}", onoff(s.charging), onoff(s.discharging)),
        );
    }
    for sw in &s.switches {
        let name = sw.label.as_deref().unwrap_or(&sw.id);
        row(&mut o, &format!("{name}:"), if sw.on { "on" } else { "off" }.to_string());
    }
    if s.charge_current_limit_a.is_some() || s.discharge_current_limit_a.is_some() {
        row(
            &mut o,
            "Limits:",
            format!(
                "chg {} / dis {}",
                opt(&s.charge_current_limit_a, " A"),
                opt(&s.discharge_current_limit_a, " A")
            ),
        );
    }
    if let Some(h) = s.time_remaining_h {
        row(&mut o, "Time remaining:", format!("{h:.1} h"));
    }
    if s.capacity_remaining_ah.is_some() {
        row(
            &mut o,
            "Capacity:",
            format!(
                "{} / {}",
                opt(&s.capacity_remaining_ah, " Ah"),
                opt(&s.capacity_full_ah, " Ah")
            ),
        );
    }
    if s.cycles.is_some() {
        row(&mut o, "Cycles:", opt(&s.cycles, ""));
    }

    // Ports (stations), labelled by their unique id + flow direction.
    for p in &s.ports {
        let state = match p.on {
            Some(true) => "on",
            Some(false) => "off",
            None => "-",
        };
        let dir = match p.direction {
            Some(PortDirection::In) => " in",
            Some(PortDirection::Out) => " out",
            Some(PortDirection::Bidir) => " io",
            None => "",
        };
        let watts = p.watts.map(|w| format!(" {w} W")).unwrap_or_default();
        let name = p.label.as_deref().unwrap_or(&p.id);
        row(&mut o, &format!("{name}:"), format!("{state}{dir}{watts}"));
    }

    // Cells (BMS)
    if !s.cells.is_empty() {
        row(&mut o, "Cells:", s.cells.len().to_string());
        if let (Some(min), Some(max), Some(d)) = (s.cell_min(), s.cell_max(), s.cell_delta()) {
            row(
                &mut o,
                "  min/max/Δ:",
                format!("{min:.3} / {max:.3} / {:.0} mV", d * 1000.0),
            );
        }
    }

    if !s.alarms.is_empty() {
        row(&mut o, "Alarms:", s.alarms.join(", "));
    }
    if let Some(sn) = &info.serial {
        row(&mut o, "Serial:", sn.clone());
    }
    o
}

fn onoff(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "on",
        Some(false) => "off",
        None => "-",
    }
}
