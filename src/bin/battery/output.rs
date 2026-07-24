//! Text/JSON rendering for the `battery` CLI.

use battery_control::{BatteryStatus, DeviceInfo, Discovered, PortDirection, SettingValue};

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
            o.insert("manufacturer".into(), serde_json::json!(info.manufacturer));
            o.insert("model".into(), serde_json::json!(info.model));
            o.insert("serial".into(), serde_json::json!(info.serial));
            o.insert("firmware".into(), serde_json::json!(info.firmware));
            o.insert("hardware".into(), serde_json::json!(info.hardware));
        }
        println!("{}", serde_json::to_string(&v).unwrap());
        return;
    }
    print!("{}", render_text(info, s));
}

/// Format a float without trailing `.0` noise.
fn num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

/// Human-ish label from an id: `"power_in"`/`"temp.mosfet"` -> `"Power in"`.
fn nice(id: &str) -> String {
    let mut c = id.replace(['_', '.'], " ");
    if let Some(f) = c.get_mut(0..1) {
        f.make_ascii_uppercase();
    }
    c
}

fn render_text(info: &DeviceInfo, s: &BatteryStatus) -> String {
    let title = info.model.clone().unwrap_or_else(|| info.backend.to_string());
    let mut o = format!("── {title} ({}) ──\n", info.backend);
    let row = |o: &mut String, k: &str, v: String| o.push_str(&format!("{k:<20} {v}\n"));

    // Readings: one row each, in the order the backend reported them.
    for sensor in &s.sensors {
        let name = sensor.label.clone().unwrap_or_else(|| nice(&sensor.id));
        let sym = sensor.unit.symbol();
        let sep = if sym.is_empty() { "" } else { " " };
        row(&mut o, &format!("{name}:"), format!("{}{sep}{sym}", num(sensor.value)));
    }

    // Switches (incl. charging/discharging).
    for sw in &s.switches {
        let name = sw.label.as_deref().unwrap_or(&sw.id);
        row(&mut o, &format!("{name}:"), if sw.on { "on" } else { "off" }.to_string());
    }

    // Settings (read/write config).
    for st in &s.settings {
        let name = st.label.as_deref().unwrap_or(&st.id);
        let val = match &st.value {
            SettingValue::Bool(b) => if *b { "on".into() } else { "off".into() },
            SettingValue::Number(n) => num(*n),
            SettingValue::Text(t) => t.clone(),
        };
        let ro = if st.writable { "" } else { " (ro)" };
        row(&mut o, &format!("{name}:"), format!("{val}{ro}"));
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
    // Static device identity (BLE Device Information Service, when available).
    if let Some(v) = &info.manufacturer {
        row(&mut o, "Manufacturer:", v.clone());
    }
    if let Some(v) = &info.serial {
        row(&mut o, "Serial:", v.clone());
    }
    if let Some(v) = &info.firmware {
        row(&mut o, "Firmware:", v.clone());
    }
    if let Some(v) = &info.hardware {
        row(&mut o, "Hardware:", v.clone());
    }
    o
}
