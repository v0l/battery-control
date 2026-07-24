//! Command builder. A command is `DFEC00 ‖ action ‖ msg_type ‖ body_len ‖ body`,
//! where `body` is compact JSON. The whole thing is then encrypted.

const PREFIX_PORTABLE: [u8; 3] = [0xDF, 0xEC, 0x00];

// Action ids.
const ACTION_OUTPUT_DC: u8 = 1;
const ACTION_OUTPUT_DC_USB: u8 = 2;
const ACTION_OUTPUT_DC_CAR: u8 = 3;
const ACTION_OUTPUT_AC: u8 = 4;
const ACTION_DEVICE_PROPERTY: u8 = 252;

// Message types.
const MSG_SET_CONTROL: u8 = 4;
const MSG_DEVICE_PROPERTY: u8 = 3;

fn build(action: u8, msg_type: u8, body: &[u8]) -> Vec<u8> {
    let mut cmd = Vec::with_capacity(PREFIX_PORTABLE.len() + 3 + body.len());
    cmd.extend_from_slice(&PREFIX_PORTABLE);
    cmd.push(action);
    cmd.push(msg_type);
    cmd.push(body.len() as u8);
    cmd.extend_from_slice(body);
    cmd
}

/// The status query (device property).
pub fn query_device_property() -> Vec<u8> {
    build(ACTION_DEVICE_PROPERTY, MSG_DEVICE_PROPERTY, b"")
}

fn set(action: u8, key: &str, on: bool) -> Vec<u8> {
    let body = format!("{{\"{key}\":{}}}", on as u8);
    build(action, MSG_SET_CONTROL, body.as_bytes())
}

pub fn set_ac_output(on: bool) -> Vec<u8> {
    set(ACTION_OUTPUT_AC, "oac", on)
}
pub fn set_dc_output(on: bool) -> Vec<u8> {
    set(ACTION_OUTPUT_DC, "odc", on)
}
pub fn set_dc_usb_output(on: bool) -> Vec<u8> {
    set(ACTION_OUTPUT_DC_USB, "odcu", on)
}
pub fn set_dc_car_output(on: bool) -> Vec<u8> {
    set(ACTION_OUTPUT_DC_CAR, "odcc", on)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_frame() {
        assert_eq!(query_device_property(), [0xDF, 0xEC, 0x00, 0xFC, 0x03, 0x00]);
    }

    #[test]
    fn set_frame() {
        let c = set_ac_output(true);
        assert_eq!(&c[..6], &[0xDF, 0xEC, 0x00, 0x04, 0x04, 9]); // body len 9
        assert_eq!(&c[6..], br#"{"oac":1}"#);
    }
}
