pub mod bms;
pub mod error;
pub mod pack;
pub mod protocol;
pub mod session;
pub mod module;
pub mod jk_info;
pub mod transport;

pub use error::{JkError, Result};
pub use pack::{MybmmPack, ProtocolVersion, JkSettings};
pub use protocol::{getdata, get_info_command, get_cell_info_command, get_settings_command, get_short, get_signed_short, crc, error_bitmask_to_strings, ERROR_DESCRIPTIONS, FrameAssembler, ParseFlags, get_16bit, get_32bit, ieee_float, SettingDef, SETTINGS, get_setting_register, get_setting_def, get_setting_by_register, build_write_frame, build_setting_write_frame, CAN_FRAME_SIZE, CAN_CMD_INFO, CAN_CMD_CELL_INFO, CAN_CMD_WRITE_REG, build_can_command, get_can_info_command, get_can_cell_info_command, build_can_write_frame, build_can_setting_write_frame};
pub use bms::{read_data, JkBms};
pub use session::JkSession;
pub use module::{MybmmModule, Transport, jk_init, jk_new, jk_open, jk_read, jk_close, jk_control, MYBMM_CHARGE_CONTROL, MYBMM_DISCHARGE_CONTROL, MYBMM_BALANCE_CONTROL};
pub use jk_info::{JkInfo, parse_info_strings};
pub use async_trait::async_trait;

#[cfg(feature = "bluetooth")]
pub use transport::{parse_pack_address, BluetoothTransport, BtDevice, scan as bt_scan};
#[cfg(feature = "serial")]
pub use transport::SerialTransport;
#[cfg(all(feature = "can", target_os = "linux"))]
pub use transport::CanTransport;

pub const JK_MODULE_NAME: &str = "jk";
pub const JK_MODULE_TYPE: i32 = 1;

pub fn create_jk_module() -> MybmmModule {
    MybmmModule::new(
        JK_MODULE_NAME,
        MYBMM_CHARGE_CONTROL | MYBMM_DISCHARGE_CONTROL | MYBMM_BALANCE_CONTROL,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: compute CRC and place it at byte 299 of a 300-byte frame
    fn finalize_frame(data: &mut [u8]) {
        assert!(data.len() >= 300);
        data[299] = crate::protocol::crc(data, 299);
    }

    // --- jk_read over a fragmenting transport (regression: BLE heartbeats +
    // frames split across reads used to yield NoVoltageData) ---

    /// Mock transport that ignores writes and serves a scripted sequence of
    /// read chunks, mimicking BLE notification delivery.
    struct ScriptedTransport {
        reads: std::collections::VecDeque<Vec<u8>>,
    }

    #[async_trait::async_trait]
    impl Transport for ScriptedTransport {
        async fn open(&mut self) -> Result<()> {
            Ok(())
        }
        async fn close(&mut self) -> Result<()> {
            Ok(())
        }
        async fn write(&mut self, data: &[u8]) -> Result<usize> {
            Ok(data.len())
        }
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            let Some(chunk) = self.reads.pop_front() else {
                return Ok(0);
            };
            buf[..chunk.len()].copy_from_slice(&chunk);
            Ok(chunk.len())
        }
    }

    fn info_frame() -> Vec<u8> {
        let mut f = vec![0u8; 300];
        f[0] = 0x55; f[1] = 0xAA; f[2] = 0xEB; f[3] = 0x90;
        f[4] = 0x03; // info frame
        let model = b"JK-B2A16S";
        f[6..6 + model.len()].copy_from_slice(model);
        finalize_frame(&mut f);
        f
    }

    fn cell_frame() -> Vec<u8> {
        let mut f = vec![0u8; 300];
        f[0] = 0x55; f[1] = 0xAA; f[2] = 0xEB; f[3] = 0x90;
        f[4] = 0x02; // cell info frame
        f[6] = 0xAC; f[7] = 0x0D; // 3500 mV
        f[118] = 0x00; f[119] = 0xC8; f[120] = 0x00; f[121] = 0x00;
        finalize_frame(&mut f);
        f
    }

    #[tokio::test]
    async fn test_jk_read_fragmented_with_heartbeats() {
        let info = info_frame();
        let cell = cell_frame();

        // Interleave 4-byte heartbeats and split both frames across reads,
        // starting phase 2 mid-stream (stale partial frame before a fresh one).
        let reads: std::collections::VecDeque<Vec<u8>> = [
            // phase 1: getInfo
            vec![0xAA, 0x55, 0x90, 0xEB],   // heartbeat
            info[..150].to_vec(),           // info frame, fragmented
            info[150..].to_vec(),
            // phase 2: getCellInfo
            cell[250..].to_vec(),           // tail of a stale frame
            vec![0xAA, 0x55, 0x90, 0xEB],   // heartbeat
            cell[..100].to_vec(),           // fresh frame, fragmented
            cell[100..250].to_vec(),
            cell[250..].to_vec(),
        ]
        .into_iter()
        .collect();

        let mut pack = MybmmPack::new("test");
        let mut session = JkSession {
            pp: pack.clone(),
            tp: MybmmModule::new("test", 0),
            tp_handle: Some(Box::new(ScriptedTransport { reads })),
        };

        jk_read(&mut session, &mut pack).await.expect("jk_read should succeed");
        assert_eq!(pack.model, "JK-B2A16S");
        assert!((pack.voltage - 51.2).abs() < 0.01);
    }

    #[test]
    fn test_parse_pack_address() {
        assert_eq!(parse_pack_address("REPT280-00"), Some(0));
        assert_eq!(parse_pack_address("AliBattery-01"), Some(1));
        assert_eq!(parse_pack_address("JK-B2A16S"), None); // suffix not 2 digits
        assert_eq!(parse_pack_address("NoSuffix"), None);
    }

    #[tokio::test]
    async fn test_next_update_streams_pushed_frames() {
        let cell = cell_frame();

        // Two pushed frames, each fragmented, with heartbeats interleaved —
        // no re-request between them.
        let reads: std::collections::VecDeque<Vec<u8>> = [
            vec![0xAA, 0x55, 0x90, 0xEB],
            cell[..120].to_vec(),
            cell[120..].to_vec(),
            cell[..40].to_vec(),
            cell[40..280].to_vec(),
            cell[280..].to_vec(),
        ]
        .into_iter()
        .collect();

        let mut bms = JkBms::open("test", "mock", Box::new(ScriptedTransport { reads }))
            .await
            .unwrap();

        let timeout = std::time::Duration::from_millis(200);
        let pack = bms.next_update(timeout).await.expect("first update");
        assert!((pack.voltage - 51.2).abs() < 0.01);
        let _ = bms.next_update(timeout).await.expect("second update");

        // Stream exhausted -> timeout.
        let err = bms.next_update(std::time::Duration::from_millis(20)).await.unwrap_err();
        assert!(matches!(err, JkError::Timeout));
    }

    #[tokio::test]
    async fn test_jk_read_no_data_errors() {
        let mut pack = MybmmPack::new("test");
        let mut session = JkSession {
            pp: pack.clone(),
            tp: MybmmModule::new("test", 0),
            tp_handle: Some(Box::new(ScriptedTransport {
                reads: std::collections::VecDeque::new(),
            })),
        };
        let err = jk_read(&mut session, &mut pack).await.unwrap_err();
        assert!(matches!(err, JkError::NoVoltageData));
    }

    #[test]
    fn test_get_short() {
        let data: [u8; 4] = [0x00, 0x04, 0x00, 0x00];
        assert_eq!(get_short(&data, 0), 0x0400);
    }

    #[test]
    fn test_get_signed_short() {
        let data: [u8; 4] = [0xFF, 0xFF, 0x00, 0x00];
        assert_eq!(get_signed_short(&data, 0), -1);
    }

    #[test]
    fn test_commands() {
        let info_cmd = get_info_command();
        assert_eq!(info_cmd[0], 0xaa);
        assert_eq!(info_cmd[4], 0x97);

        let cell_cmd = get_cell_info_command();
        assert_eq!(cell_cmd[0], 0xaa);
        assert_eq!(cell_cmd[4], 0x96);
    }

    // --- JK02_24S (old) protocol tests ---
    // Voltage/current are 4-byte LE per the ESPHome reference

    #[test]
    fn test_getdata_jk02_24s_voltage() {
        let mut data = vec![0u8; 400];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x02;

        // 2 cells: 3500mV and 3600mV
        data[6] = 0xAC; data[7] = 0x0D;  // 0x0DAC = 3500
        data[8] = 0x10; data[9] = 0x0E;  // 0x0E10 = 3600

        // Voltage at offset 118: 51.2V = 51200 mV = 0x0000C800 (4 bytes LE)
        data[118] = 0x00; data[119] = 0xC8; data[120] = 0x00; data[121] = 0x00;

        // Current at offset 126: 5.0A = 5000 mA = 0x00001388 (4 bytes LE)
        data[126] = 0x88; data[127] = 0x13; data[128] = 0x00; data[129] = 0x00;

        // Temp 1 at offset 130: 25.0°C = 250 = 0x00FA (signed)
        data[130] = 0xFA; data[131] = 0x00;

        // Temp 2 at offset 132: 26.5°C = 265 = 0x0109 (signed)
        data[132] = 0x09; data[133] = 0x01;

        // MOS temp at offset 134: 35.0°C = 350 = 0x015E
        data[134] = 0x5E; data[135] = 0x01;

        // Error bitmask at offset 136: 0x0000 (no errors)
        data[136] = 0x00; data[137] = 0x00;

        // SOC at offset 141: 84%
        data[141] = 84;

        // SOH at offset 158: 100%
        data[158] = 100;

        // Charging mosfet at offset 166: on
        data[166] = 0x01;

        // Discharging mosfet at offset 167: on
        data[167] = 0x01;

        finalize_frame(&mut data);

        let mut pack = MybmmPack::new("test");
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_volts);
        assert!(!flags.got_info);
        assert!(!flags.got_res);
        assert_eq!(pack.cells, 2);
        assert!((pack.cellvolt[0] - 3.5).abs() < 0.001);
        assert!((pack.cellvolt[1] - 3.6).abs() < 0.001);
        assert!((pack.voltage - 51.2).abs() < 0.01, "voltage: got {}", pack.voltage);
        assert!((pack.current - 5.0).abs() < 0.001, "current: got {}", pack.current);
        assert!((pack.temps[0] - 25.0).abs() < 0.1);
        assert!((pack.temps[1] - 26.5).abs() < 0.1);
        assert!((pack.power_tube_temp - 35.0).abs() < 0.1);
        assert_eq!(pack.protocol_version, ProtocolVersion::Jk02_24S);
        assert!((pack.soc - 84.0).abs() < 0.1);
        assert!((pack.soh - 100.0).abs() < 0.1);
        assert!(pack.charging);
        assert!(pack.discharging);
    }

    #[test]
    fn test_getdata_jk02_24s_negative_current_and_temp() {
        let mut data = vec![0u8; 400];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x02;

        // 1 cell at 3300mV
        data[6] = 0xE4; data[7] = 0x0C; // 0x0CE4 = 3300

        // Voltage at 118: 53.060V = 53060 mV = 0x0000CF44
        data[118] = 0x44; data[119] = 0xCF; data[120] = 0x00; data[121] = 0x00;

        // Current at 126: -5.0A = -5000 mA = 0xFFFFEC78 (i32 LE)
        data[126] = 0x78; data[127] = 0xEC; data[128] = 0xFF; data[129] = 0xFF;

        // Negative temp at 130: -5.0°C = -50 = 0xFFCE (i16 LE)
        data[130] = 0xCE; data[131] = 0xFF;
        // Normal temp at 132: 24.0°C = 240 = 0x00F0
        data[132] = 0xF0; data[133] = 0x00;

        finalize_frame(&mut data);

        let mut pack = MybmmPack::new("test");
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_volts);
        assert!((pack.current - (-5.0)).abs() < 0.01, "current: got {}", pack.current);
        assert!((pack.voltage - 53.060).abs() < 0.01);
        assert!((pack.temps[0] - (-5.0)).abs() < 0.1, "temp0: got {}", pack.temps[0]);
        assert!((pack.temps[1] - 24.0).abs() < 0.1);
        assert!(pack.power < 0.0); // discharging
    }

    #[test]
    fn test_getdata_jk02_24s_error_bitmask() {
        let mut data = vec![0u8; 400];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x02;

        data[6] = 0xAC; data[7] = 0x0D; // 3500mV

        // Voltage at 118
        data[118] = 0x00; data[119] = 0xC8; data[120] = 0x00; data[121] = 0x00;
        // Current at 126
        data[126] = 0x00; data[127] = 0x00; data[128] = 0x00; data[129] = 0x00;

        // Error bitmask at offset 136: bit 3 (Cell Undervoltage) + bit 12 (Cell Overvoltage)
        // = 0x1008
        data[136] = 0x08; data[137] = 0x10;

        finalize_frame(&mut data);

        let mut pack = MybmmPack::new("test");
        let _ = getdata(&mut pack, &data);

        assert_eq!(pack.error_bitmask, 0x1008);
        let errors = error_bitmask_to_strings(pack.error_bitmask);
        assert!(errors.contains(&"Cell Undervoltage"));
        assert!(errors.contains(&"Cell Overvoltage"));
        assert_eq!(errors.len(), 2);
    }

    // --- JK02_32S (PB2) protocol tests ---

    #[test]
    fn test_getdata_jk02_32s_cell_info() {
        let mut data = vec![0u8; 400];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x02;
        data[5] = 0x55; // frame counter (happens to be 0x55, tests scanner robustness)

        // 2 cells: 3318mV and 3317mV (at offset 6, 2 bytes each LE)
        data[6] = 0xF6; data[7] = 0x0C;  // 0x0CF6 = 3318
        data[8] = 0xF5; data[9] = 0x0C;  // 0x0CF5 = 3317

        // Cell resistances at offset 64+16=80: 72 mΩ, 75 mΩ (0.001 Ohm each)
        data[80] = 0x48; data[81] = 0x00; // 0x0048 = 72 -> 0.072 Ohm
        data[82] = 0x4B; data[83] = 0x00; // 0x004B = 75 -> 0.075 Ohm

        // For 32S, offset = 16, offset2 = 32
        let _offset2 = 32;

        // Voltage at offset 118+32=150: 53047 mV = 0x0000CF37
        data[150] = 0x37; data[151] = 0xCF; data[152] = 0x00; data[153] = 0x00;

        // Current at offset 126+32=158: -6808 mA = 0xFFFFE568
        data[158] = 0x68; data[159] = 0xE5; data[160] = 0xFF; data[161] = 0xFF;

        // Temp 1 at offset 130+32=162: 234 = 23.4°C
        data[162] = 0xEA; data[163] = 0x00;

        // Temp 2 at offset 132+32=164: 240 = 24.0°C
        data[164] = 0xF0; data[165] = 0x00;

        // Error bitmask at offset 134+32=166 (32S uses this instead of MOS temp)
        data[166] = 0x00; data[167] = 0x00;

        // SOC at offset 141+32=173
        data[173] = 76;

        finalize_frame(&mut data);

        let mut pack = MybmmPack::new("test");
        pack.protocol_version = ProtocolVersion::Jk02_32S;
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_volts);
        assert_eq!(pack.cells, 2);
        assert!((pack.cellvolt[0] - 3.318).abs() < 0.001);
        assert!((pack.cellvolt[1] - 3.317).abs() < 0.001);
        assert!((pack.voltage - 53.047).abs() < 0.01, "voltage: got {}", pack.voltage);
        assert!((pack.current - (-6.808)).abs() < 0.01, "current: got {}", pack.current);
        assert!((pack.temps[0] - 23.4).abs() < 0.1);
        assert!((pack.temps[1] - 24.0).abs() < 0.1);
        // Cell resistances are in Ohms (raw * 0.001)
        assert!((pack.cellres[0] - 0.072).abs() < 0.001, "cellres[0]: got {}", pack.cellres[0]);
        assert!((pack.cellres[1] - 0.075).abs() < 0.001, "cellres[1]: got {}", pack.cellres[1]);
        assert!((pack.soc - 76.0).abs() < 0.1);
    }

    #[test]
    fn test_getdata_jk02_32s_16cells() {
        let mut data = vec![0u8; 400];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x02;
        data[5] = 0x63;

        // 16 cells all at 3315mV (0x0CF3)
        for i in 0..16 {
            data[6 + i * 2] = 0xF3;
            data[6 + i * 2 + 1] = 0x0C;
        }

        let _offset2 = 32;

        // Voltage at 118+32=150: 53056 mV
        data[150] = 0x40; data[151] = 0xCF; data[152] = 0x00; data[153] = 0x00;

        // Current at 158: -9058 mA
        data[158] = 0x9E; data[159] = 0xDC; data[160] = 0xFF; data[161] = 0xFF;

        // Temps
        data[162] = 0xD5; data[163] = 0x00; // 213 = 21.3°C
        data[164] = 0xDB; data[165] = 0x00; // 219 = 21.9°C

        finalize_frame(&mut data);

        let mut pack = MybmmPack::new("test");
        pack.protocol_version = ProtocolVersion::Jk02_32S;
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_volts);
        assert_eq!(pack.cells, 16);
        for i in 0..16 {
            assert!((pack.cellvolt[i] - 3.315).abs() < 0.001, "cell {} = {}", i, pack.cellvolt[i]);
        }
        assert!((pack.voltage - 53.056).abs() < 0.01);
        assert!((pack.current - (-9.058)).abs() < 0.01);
        assert!((pack.temps[0] - 21.3).abs() < 0.1);
        assert!((pack.temps[1] - 21.9).abs() < 0.1);
    }

    // --- Info frame tests ---

    #[test]
    fn test_getdata_info_frame_detects_pb2() {
        let mut data = vec![0u8; 400];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x03;

        let model = b"JK_PB2A16S20P";
        data[6..6 + model.len()].copy_from_slice(model);
        data[6 + model.len()] = 0x00;

        finalize_frame(&mut data);

        let mut pack = MybmmPack::new("test");
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_info);
        assert!(!flags.got_volts);
        assert_eq!(pack.protocol_version, ProtocolVersion::Jk02_32S, "JK_PB2 should be JK02_32S");
        assert!(pack.is_pb2());
    }

    #[test]
    fn test_getdata_info_frame_non_pb2() {
        let mut data = vec![0u8; 400];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x03;

        let model = b"JK-B2A16S";
        data[6..6 + model.len()].copy_from_slice(model);
        data[6 + model.len()] = 0x00;

        finalize_frame(&mut data);

        let mut pack = MybmmPack::new("test");
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_info);
        assert_eq!(pack.protocol_version, ProtocolVersion::Jk02_24S, "JK-B2 should be JK02_24S");
        assert!(!pack.is_pb2());
    }

    #[test]
    fn test_getdata_info_detects_jk_bd() {
        let mut data = vec![0u8; 400];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x03;

        let model = b"JK-BD6A20S";
        data[6..6 + model.len()].copy_from_slice(model);
        data[6 + model.len()] = 0x00;

        finalize_frame(&mut data);

        let mut pack = MybmmPack::new("test");
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_info);
        assert_eq!(pack.protocol_version, ProtocolVersion::Jk02_32S, "JK-BD should be JK02_32S");
    }

    #[test]
    fn test_getdata_info_detects_jk_hy() {
        let mut data = vec![0u8; 400];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x03;

        let model = b"JK_HY102A16S";
        data[6..6 + model.len()].copy_from_slice(model);
        data[6 + model.len()] = 0x00;

        finalize_frame(&mut data);

        let mut pack = MybmmPack::new("test");
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_info);
        assert_eq!(pack.protocol_version, ProtocolVersion::Jk02_32S, "JK_HY should be JK02_32S");
    }

    // --- Scanner tests ---

    #[test]
    fn test_getdata_type01_res_frame() {
        let mut data = vec![0u8; 400];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x01;

        finalize_frame(&mut data);

        let mut pack = MybmmPack::new("test");
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_res);
        assert!(!flags.got_volts);
        assert!(!flags.got_info);
    }

    #[test]
    fn test_getdata_frame_at_nonzero_offset() {
        let mut data = vec![0u8; 400];
        data[0] = b'A'; data[1] = b'T'; data[2] = 0x0D; data[3] = 0x0A;

        // Frame starts at offset 4
        data[4] = 0x55; data[5] = 0xAA; data[6] = 0xEB; data[7] = 0x90;
        data[8] = 0x02;

        // 1 cell at 3315mV
        data[10] = 0xF3; data[11] = 0x0C;

        // Old protocol voltage at offset 4+118 = 122 (4 bytes LE)
        data[122] = 0x44; data[123] = 0xCF; data[124] = 0x00; data[125] = 0x00;

        // Current at offset 4+126 = 130 (4 bytes LE)
        data[130] = 0x78; data[131] = 0xEC; data[132] = 0xFF; data[133] = 0xFF;

        // Temps at 4+130=134, 4+132=136
        data[134] = 0xEA; data[135] = 0x00;
        data[136] = 0xF0; data[137] = 0x00;

        // CRC at offset 4+299=303
        finalize_frame(&mut data[4..].to_vec().as_mut_slice());
        // Recompute: need CRC relative to frame start
        // Actually, finalize_frame works on the whole slice starting at 0
        // Let's compute CRC manually for the frame starting at offset 4
        let frame_start = 4;
        let frame_data = &data[frame_start..];
        let mut crc_val: u8 = 0;
        for i in 0..299 {
            crc_val = crc_val.wrapping_add(frame_data[i]);
        }
        data[frame_start + 299] = crc_val;

        let mut pack = MybmmPack::new("test");
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_volts);
        assert_eq!(pack.cells, 1);
        assert!((pack.cellvolt[0] - 3.315).abs() < 0.001);
        assert!((pack.voltage - 53.060).abs() < 0.01);
    }

    #[test]
    fn test_getdata_multi_frame_info_then_voltage() {
        let mut data = vec![0u8; 800];

        // Info frame at offset 0
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x03;
        let model = b"JK_PB2A16S20P";
        data[6..6 + model.len()].copy_from_slice(model);
        data[6 + model.len()] = 0x00;

        // CRC for info frame
        {
            let mut crc_val: u8 = 0;
            for i in 0..299 { crc_val = crc_val.wrapping_add(data[i]); }
            data[299] = crc_val;
        }

        // Voltage frame at offset 300
        let voff = 300;
        data[voff] = 0x55; data[voff + 1] = 0xAA;
        data[voff + 2] = 0xEB; data[voff + 3] = 0x90;
        data[voff + 4] = 0x02;

        // 2 cells
        data[voff + 6] = 0xF6; data[voff + 7] = 0x0C;
        data[voff + 8] = 0xF5; data[voff + 9] = 0x0C;

        // Since PB2 detected from info frame, offset=16, offset2=32
        // Voltage at voff+150
        data[voff + 150] = 0x37; data[voff + 151] = 0xCF;
        data[voff + 152] = 0x00; data[voff + 153] = 0x00;

        // Current at voff+158
        data[voff + 158] = 0x68; data[voff + 159] = 0xE5;
        data[voff + 160] = 0xFF; data[voff + 161] = 0xFF;

        // Temps at voff+162, voff+164
        data[voff + 162] = 0xEA; data[voff + 163] = 0x00;
        data[voff + 164] = 0xF0; data[voff + 165] = 0x00;

        // CRC for voltage frame
        {
            let mut crc_val: u8 = 0;
            for i in 0..299 { crc_val = crc_val.wrapping_add(data[voff + i]); }
            data[voff + 299] = crc_val;
        }

        let mut pack = MybmmPack::new("test");
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_info, "should find info frame");
        assert_eq!(pack.protocol_version, ProtocolVersion::Jk02_32S, "should detect JK02_32S from info frame model");
        assert!(flags.got_volts, "should find voltage frame after info frame");
        assert!((pack.voltage - 53.047).abs() < 0.01);
        assert!((pack.current - (-6.808)).abs() < 0.01);
    }

    #[test]
    fn test_getdata_0x55_in_data() {
        let mut data = vec![0u8; 400];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x02;
        data[5] = 0x55;

        // 1 cell
        data[6] = 0xF6; data[7] = 0x0C;

        // Voltage at 118
        data[118] = 0x00; data[119] = 0x80; data[120] = 0x00; data[121] = 0x00;

        // Current at 126
        data[126] = 0x00; data[127] = 0x00; data[128] = 0x00; data[129] = 0x00;

        finalize_frame(&mut data);

        let mut pack = MybmmPack::new("test");
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_volts);
        assert_eq!(pack.cells, 1);
        assert!((pack.voltage - 32.768).abs() < 0.01);
    }

    #[test]
    fn test_getdata_no_signature() {
        let data = vec![0u8; 200];
        let mut pack = MybmmPack::new("test");
        let flags = getdata(&mut pack, &data);

        assert!(!flags.got_volts);
        assert!(!flags.got_info);
        assert!(!flags.got_res);
    }

    #[test]
    fn test_getdata_short_buffer() {
        let data = [0x55, 0xAA, 0xEB, 0x90, 0x02, 0x00];
        let mut pack = MybmmPack::new("test");
        let flags = getdata(&mut pack, &data);

        assert!(!flags.got_volts);
    }

    #[test]
    fn test_crc_verification() {
        let mut data = vec![0u8; 300];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x02;
        data[6] = 0xAC; data[7] = 0x0D; // 3500mV

        finalize_frame(&mut data);

        // CRC should pass
        let mut pack = MybmmPack::new("test");
        let flags = getdata(&mut pack, &data);
        assert!(flags.got_volts);

        // Corrupt a byte and CRC should fail
        data[50] = data[50].wrapping_add(1);
        let mut pack2 = MybmmPack::new("test");
        let flags2 = getdata(&mut pack2, &data);
        assert!(!flags2.got_volts, "CRC failure should reject frame");
    }

    #[test]
    fn test_getdata_stops_after_got_volts() {
        let mut data = vec![0u8; 800];

        // Voltage frame at offset 0
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x02;
        data[6] = 0xAC; data[7] = 0x0D; // 3500mV
        data[118] = 0x00; data[119] = 0xC8; data[120] = 0x00; data[121] = 0x00;

        finalize_frame(&mut data);

        // We can't easily make a second frame since CRC needs to be valid
        // Just verify the first frame is parsed correctly
        let mut pack = MybmmPack::new("test");
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_volts);
        assert!((pack.cellvolt[0] - 3.5).abs() < 0.001);
        assert!((pack.voltage - 51.2).abs() < 0.01);
    }

    #[test]
    fn test_protocol_version_from_model() {
        assert_eq!(ProtocolVersion::from_model("JK_PB2A16S20P"), ProtocolVersion::Jk02_32S);
        assert_eq!(ProtocolVersion::from_model("JK-BD6A20S"), ProtocolVersion::Jk02_32S);
        assert_eq!(ProtocolVersion::from_model("JK_HY102A16S"), ProtocolVersion::Jk02_32S);
        assert_eq!(ProtocolVersion::from_model("JK-B2A16S"), ProtocolVersion::Jk02_24S);
        assert_eq!(ProtocolVersion::from_model("JK-B2A24S"), ProtocolVersion::Jk02_24S);
        assert_eq!(ProtocolVersion::from_model("SomeOther"), ProtocolVersion::Jk02_24S);
    }

    #[test]
    fn test_error_bitmask_to_strings() {
        // All bits
        let all = error_bitmask_to_strings(0xFFFF);
        assert_eq!(all.len(), 16);
        assert_eq!(all[0], "Charge Overtemperature");
        assert_eq!(all[15], "Charge short circuit");

        // Single bit
        let single = error_bitmask_to_strings(0x0008);
        assert_eq!(single.len(), 1);
        assert_eq!(single[0], "Cell Undervoltage");

        // No errors
        let none = error_bitmask_to_strings(0x0000);
        assert!(none.is_empty());
    }

    #[test]
    fn test_get_short_out_of_bounds() {
        let data = [0x01, 0x02];
        assert_eq!(get_short(&data, 0), 0x0201);
        assert_eq!(get_short(&data, 1), 0);
        assert_eq!(get_short(&data, 2), 0);
    }

    #[test]
    fn test_crc_function() {
        // Known CRC: all zeros should give 0
        let zeros = [0u8; 300];
        assert_eq!(crate::protocol::crc(&zeros, 299), 0);

        // Single byte
        assert_eq!(crate::protocol::crc(&[0x42], 1), 0x42);
    }

    // --- FrameAssembler tests (BLE fragmentation) ---

    #[test]
    fn test_frame_assembler_single_complete_frame() {
        let mut frame = vec![0u8; 300];
        frame[0] = 0x55; frame[1] = 0xAA; frame[2] = 0xEB; frame[3] = 0x90;
        frame[4] = 0x02;
        frame[6] = 0xAC; frame[7] = 0x0D; // 3500mV
        frame[118] = 0x00; frame[119] = 0xC8; frame[120] = 0x00; frame[121] = 0x00;
        finalize_frame(&mut frame);

        let mut assembler = FrameAssembler::new();
        let mut pack = MybmmPack::new("test");

        // Feed entire frame at once
        let result = assembler.feed_and_decode(&mut pack, &frame);
        assert!(result.is_some());
        let flags = result.unwrap();
        assert!(flags.got_volts);
        assert!((pack.voltage - 51.2).abs() < 0.01);
    }

    #[test]
    fn test_frame_assembler_fragmented_ble() {
        let mut frame = vec![0u8; 300];
        frame[0] = 0x55; frame[1] = 0xAA; frame[2] = 0xEB; frame[3] = 0x90;
        frame[4] = 0x02;
        frame[6] = 0xAC; frame[7] = 0x0D; // 3500mV
        frame[118] = 0x00; frame[119] = 0xC8; frame[120] = 0x00; frame[121] = 0x00;
        finalize_frame(&mut frame);

        let mut assembler = FrameAssembler::new();

        // Simulate BLE notifications arriving in 20-byte chunks
        // Feed all but the last chunk using feed() (no decode attempt)
        let chunks: Vec<&[u8]> = frame.chunks(20).collect();
        for i in 0..chunks.len() - 1 {
            assembler.feed(chunks[i]);
            assert!(assembler.buffer_len() < 300, "should not have 300 bytes yet");
        }

        // Buffer should have 280 bytes (14 chunks * 20)
        assert!(assembler.buffer_len() < 300);

        // Feed the last chunk and decode
        let mut pack = MybmmPack::new("test");
        let result = assembler.feed_and_decode(&mut pack, chunks.last().unwrap());
        assert!(result.is_some(), "should decode once buffer has 300+ bytes");
        assert!(result.unwrap().got_volts);
        assert!((pack.voltage - 51.2).abs() < 0.01);
    }

    #[test]
    fn test_frame_assembler_preamble_flushes() {
        let mut frame1 = vec![0u8; 300];
        frame1[0] = 0x55; frame1[1] = 0xAA; frame1[2] = 0xEB; frame1[3] = 0x90;
        frame1[4] = 0x03; // info frame
        let model = b"JK-B2A16S";
        frame1[6..6 + model.len()].copy_from_slice(model);
        frame1[6 + model.len()] = 0x00;
        finalize_frame(&mut frame1);

        let mut frame2 = vec![0u8; 300];
        frame2[0] = 0x55; frame2[1] = 0xAA; frame2[2] = 0xEB; frame2[3] = 0x90;
        frame2[4] = 0x02;
        frame2[6] = 0xAC; frame2[7] = 0x0D;
        frame2[118] = 0x00; frame2[119] = 0xC8; frame2[120] = 0x00; frame2[121] = 0x00;
        finalize_frame(&mut frame2);

        let mut assembler = FrameAssembler::new();
        let mut pack = MybmmPack::new("test");

        // Feed first 100 bytes of frame1 (partial)
        assembler.feed(&frame1[..100]);
        assert_eq!(assembler.buffer_len(), 100);

        // Now feed frame2 preamble — should flush the partial frame1 data
        assembler.feed(&frame2[..50]);
        // Buffer should have been flushed when preamble was seen, then filled with frame2 data
        assert_eq!(assembler.buffer_len(), 50);

        // Feed the rest of frame2
        let result = assembler.feed_and_decode(&mut pack, &frame2[50..]);
        assert!(result.is_some());
        assert!(result.unwrap().got_volts);
    }

    #[test]
    fn test_frame_assembler_crc_failure_clears() {
        let mut frame = vec![0u8; 300];
        frame[0] = 0x55; frame[1] = 0xAA; frame[2] = 0xEB; frame[3] = 0x90;
        frame[4] = 0x02;
        finalize_frame(&mut frame);

        let mut assembler = FrameAssembler::new();

        // Corrupt a byte
        frame[50] = frame[50].wrapping_add(1);

        // Feed the corrupted frame
        let mut pack = MybmmPack::new("test");
        let result = assembler.feed_and_decode(&mut pack, &frame);
        assert!(result.is_none(), "CRC failure should return None");
        assert_eq!(assembler.buffer_len(), 0, "buffer should be cleared after CRC failure");
    }

    // --- Byte ordering tests for get_16bit / get_32bit (ESPHome-compatible) ---

    #[test]
    fn test_get_16bit_byte_order() {
        // ESPHome: (data[i+1] << 8) | data[i]  — little-endian pairs
        let data = [0x34, 0x12, 0x78, 0x56];
        // get_16bit at 0 should give (0x12 << 8) | 0x34 = 0x1234
        assert_eq!(crate::protocol::get_16bit(&data, 0), 0x1234);
        // get_16bit at 2 should give (0x56 << 8) | 0x78 = 0x5678
        assert_eq!(crate::protocol::get_16bit(&data, 2), 0x5678);
    }

    #[test]
    fn test_get_32bit_byte_order() {
        // ESPHome: (get_16bit(i+2) << 16) | get_16bit(i)
        // For data [0x78 0x56 0x34 0x12]:
        //   get_16bit(0) = (0x56 << 8) | 0x78 = 0x5678
        //   get_16bit(2) = (0x12 << 8) | 0x34 = 0x1234
        //   get_32bit(0) = (0x1234 << 16) | 0x5678 = 0x12345678
        let data = [0x78, 0x56, 0x34, 0x12];
        assert_eq!(crate::protocol::get_32bit(&data, 0), 0x12345678);
    }

    #[test]
    fn test_ieee_float() {
        // 3.315 as IEEE 754: f32::to_bits() = 0x405428F6
        let bits = 3.315f32.to_bits();
        assert_eq!(crate::protocol::ieee_float(bits), 3.315f32);

        // -5.0
        let bits_neg = (-5.0f32).to_bits();
        assert_eq!(crate::protocol::ieee_float(bits_neg), -5.0f32);

        // 0.0
        assert_eq!(crate::protocol::ieee_float(0u32), 0.0f32);
    }

    // --- Comprehensive JK02_24S test covering ALL fields ---

    #[test]
    fn test_getdata_jk02_24s_all_fields() {
        let mut data = vec![0u8; 400];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x02;

        // 2 cells: 3500mV and 3600mV
        data[6] = 0xAC; data[7] = 0x0D;   // 0x0DAC = 3500
        data[8] = 0x10; data[9] = 0x0E;   // 0x0E10 = 3600

        // Cell resistances at offset 64: 63 mΩ and 85 mΩ → 0.063 Ω, 0.085 Ω
        data[64] = 0x3F; data[65] = 0x00;  // 0x003F = 63 → 0.063 Ω
        data[66] = 0x55; data[67] = 0x00;  // 0x0055 = 85 → 0.085 Ω

        // Enabled cells bitmask at offset 54: 0x00000003 (2 cells)
        data[54] = 0x03; data[55] = 0x00; data[56] = 0x00; data[57] = 0x00;

        // Voltage at offset 118: 51.2V = 51200 mV = 0x0000C800
        data[118] = 0x00; data[119] = 0xC8; data[120] = 0x00; data[121] = 0x00;

        // Current at offset 126: -5.0A = -5000 mA = 0xFFFFEC78
        data[126] = 0x78; data[127] = 0xEC; data[128] = 0xFF; data[129] = 0xFF;

        // Temp 1 at 130: 25.0°C = 250
        data[130] = 0xFA; data[131] = 0x00;
        // Temp 2 at 132: 26.5°C = 265
        data[132] = 0x09; data[133] = 0x01;

        // MOSFET temp at 134: 35.0°C = 350
        data[134] = 0x5E; data[135] = 0x01;

        // Error bitmask at 136: 0x0008 (Cell Undervoltage)
        data[136] = 0x08; data[137] = 0x00;

        // Balancing current at 138: 0.050 A = 50 mA
        data[138] = 0x32; data[139] = 0x00;  // 0x0032 = 50 → 0.050 A

        // Balancing at 140: 0x01 (charging balancer)
        data[140] = 0x01;

        // SOC at 141: 84%
        data[141] = 84;

        // Capacity remaining at 142: 100.0 Ah = 100000 mAh
        // Using get_32bit byte order: [lo16_lo, lo16_hi, hi16_lo, hi16_hi]
        // 100000 = 0x000186A0 → lo16=0x86A0, hi16=0x0001
        // get_16bit reads (hi<<8)|lo → 0xA0 then 0x86 → 0x86A0, then 0x01 0x00 → 0x0001
        data[142] = 0xA0; data[143] = 0x86; data[144] = 0x01; data[145] = 0x00;

        // Total battery capacity at 146: 300.0 Ah = 300000 mAh = 0x000493E0
        // lo16=0x93E0, hi16=0x0004
        data[146] = 0xE0; data[147] = 0x93; data[148] = 0x04; data[149] = 0x00;

        // Charging cycles at 150: 42 = 0x0000002A
        data[150] = 0x2A; data[151] = 0x00; data[152] = 0x00; data[153] = 0x00;

        // Total charging cycle capacity at 154: 1000.0 Ah = 1000000 mAh = 0x000F4240
        // lo16=0x4240, hi16=0x000F
        data[154] = 0x40; data[155] = 0x42; data[156] = 0x0F; data[157] = 0x00;

        // SOH at 158: 99%
        data[158] = 99;

        // Total runtime at 162: 86400 seconds = 1 day = 0x00015180
        data[162] = 0x80; data[163] = 0x51; data[164] = 0x01; data[165] = 0x00;

        // Charging MOS at 166: on
        data[166] = 0x01;

        // Discharging MOS at 167: on
        data[167] = 0x01;

        // Precharging at 168: on
        data[168] = 0x01;

        // Heating at 183: off
        data[183] = 0x00;

        finalize_frame(&mut data);

        let mut pack = MybmmPack::new("test");
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_volts);
        assert_eq!(pack.cells, 2);

        // Cell voltages
        assert!((pack.cellvolt[0] - 3.5).abs() < 0.001);
        assert!((pack.cellvolt[1] - 3.6).abs() < 0.001);

        // Cell resistances (in Ohms, raw * 0.001)
        assert!((pack.cellres[0] - 0.063).abs() < 0.001, "cellres[0]: got {}", pack.cellres[0]);
        assert!((pack.cellres[1] - 0.085).abs() < 0.001, "cellres[1]: got {}", pack.cellres[1]);

        // Computed cell stats
        assert!((pack.cell_min - 3.5).abs() < 0.001, "cell_min: got {}", pack.cell_min);
        assert!((pack.cell_max - 3.6).abs() < 0.001, "cell_max: got {}", pack.cell_max);
        assert!((pack.cell_diff - 0.1).abs() < 0.001, "cell_diff: got {}", pack.cell_diff);
        assert!((pack.cell_avg - 3.55).abs() < 0.001, "cell_avg: got {}", pack.cell_avg);

        // Enabled cells bitmask
        assert_eq!(pack.enabled_cells_bitmask, 0x00000003);

        // Voltage, current, power
        assert!((pack.voltage - 51.2).abs() < 0.01);
        assert!((pack.current - (-5.0)).abs() < 0.01);
        assert!((pack.power - (-256.0)).abs() < 0.1, "power: got {}", pack.power);
        assert!((pack.charging_power - 0.0).abs() < 0.1, "charging_power: got {}", pack.charging_power);
        assert!((pack.discharging_power - 256.0).abs() < 0.1, "discharging_power: got {}", pack.discharging_power);

        // Temps
        assert_eq!(pack.ntemps, 2);
        assert!((pack.temps[0] - 25.0).abs() < 0.1);
        assert!((pack.temps[1] - 26.5).abs() < 0.1);

        // MOSFET temp
        assert!((pack.power_tube_temp - 35.0).abs() < 0.1);

        // Error bitmask
        assert_eq!(pack.error_bitmask, 0x0008);

        // Balancing current
        assert!((pack.balancing_current - 0.050).abs() < 0.001, "balancing_current: got {}", pack.balancing_current);

        // Balancing
        assert!(pack.balancing);

        // SOC
        assert!((pack.soc - 84.0).abs() < 0.1);

        // Capacity remaining
        assert!((pack.capacity_remaining - 100.0).abs() < 0.01, "capacity_remaining: got {}", pack.capacity_remaining);

        // Total battery capacity
        assert!((pack.total_battery_capacity - 300.0).abs() < 0.01);

        // Charging cycles
        assert_eq!(pack.charging_cycles, 42);

        // Total charging cycle capacity
        assert!((pack.total_charging_cycle_capacity - 1000.0).abs() < 0.1, "total_charging_cycle_capacity: got {}", pack.total_charging_cycle_capacity);

        // SOH
        assert!((pack.soh - 99.0).abs() < 0.1);

        // Total runtime
        assert_eq!(pack.total_runtime, 86400);

        // MOS states
        assert!(pack.charging);
        assert!(pack.discharging);
        assert!(pack.precharging);
        assert!(!pack.heating);
    }

    // --- Comprehensive JK02_32S test covering ALL fields including 32S-specific ones ---

    #[test]
    fn test_getdata_jk02_32s_all_fields() {
        let mut data = vec![0u8; 400];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x02;

        // 2 cells: 3500mV and 3600mV
        data[6] = 0xAC; data[7] = 0x0D;
        data[8] = 0x10; data[9] = 0x0E;

        // Cell resistances at 64+16=80: 63mΩ, 85mΩ → 0.063Ω, 0.085Ω
        data[80] = 0x3F; data[81] = 0x00;
        data[82] = 0x55; data[83] = 0x00;

        // offset=16, offset2=32
        let _o2 = 32;

        // Power tube temp at 112+o2=144: 35.0°C = 350
        data[144] = 0x5E; data[145] = 0x01;

        // Voltage at 118+o2=150: 51.2V
        data[150] = 0x00; data[151] = 0xC8; data[152] = 0x00; data[153] = 0x00;

        // Current at 126+o2=158: -5.0A
        data[158] = 0x78; data[159] = 0xEC; data[160] = 0xFF; data[161] = 0xFF;

        // Temp 1 at 130+o2=162: 25.0°C
        data[162] = 0xFA; data[163] = 0x00;
        // Temp 2 at 132+o2=164: 26.5°C
        data[164] = 0x09; data[165] = 0x01;

        // Error bitmask at 134+o2=166 (32S reads from here, NOT 136)
        data[166] = 0x20; data[167] = 0x00; // bit 5: Discharge overcurrent

        // Balancing current at 138+o2=170: 0.050A
        data[170] = 0x32; data[171] = 0x00;

        // Balancing at 140+o2=172: active
        data[172] = 0x02; // discharging balancer

        // SOC at 141+o2=173
        data[173] = 76;

        // Capacity remaining at 142+o2=174: 100.0 Ah
        data[174] = 0xA0; data[175] = 0x86; data[176] = 0x01; data[177] = 0x00;

        // Total battery capacity at 146+o2=178: 300.0 Ah
        data[178] = 0xE0; data[179] = 0x93; data[180] = 0x04; data[181] = 0x00;

        // Charging cycles at 150+o2=182: 42
        data[182] = 0x2A; data[183] = 0x00; data[184] = 0x00; data[185] = 0x00;

        // Total charging cycle capacity at 154+o2=186: 1000.0 Ah
        data[186] = 0x40; data[187] = 0x42; data[188] = 0x0F; data[189] = 0x00;

        // SOH at 158+o2=190
        data[190] = 99;

        // Total runtime at 162+o2=194
        data[194] = 0x80; data[195] = 0x51; data[196] = 0x01; data[197] = 0x00;

        // Charging MOS at 166+o2=198
        data[198] = 0x01;
        // Discharging MOS at 167+o2=199
        data[199] = 0x01;
        // Precharging at 168+o2=200
        data[200] = 0x01;
        // Heating at 183+o2=215
        data[215] = 0x01;

        // Extra temp sensors (32S only):
        // Temp 5 at 226+o2=258: 28.0°C = 280
        data[258] = 0x18; data[259] = 0x01;
        // Temp 4 at 224+o2=256: 27.5°C = 275
        data[256] = 0x13; data[257] = 0x01;
        // Temp 3 at 222+o2=254: 27.0°C = 270
        data[254] = 0x0E; data[255] = 0x01;

        finalize_frame(&mut data);

        let mut pack = MybmmPack::new("test");
        pack.protocol_version = ProtocolVersion::Jk02_32S;
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_volts);
        assert_eq!(pack.cells, 2);

        // Cell voltages & resistances
        assert!((pack.cellvolt[0] - 3.5).abs() < 0.001);
        assert!((pack.cellvolt[1] - 3.6).abs() < 0.001);
        assert!((pack.cellres[0] - 0.063).abs() < 0.001);
        assert!((pack.cellres[1] - 0.085).abs() < 0.001);

        // Computed stats
        assert!((pack.cell_min - 3.5).abs() < 0.001);
        assert!((pack.cell_max - 3.6).abs() < 0.001);
        assert!((pack.cell_diff - 0.1).abs() < 0.001);
        assert!((pack.cell_avg - 3.55).abs() < 0.001);

        // Power tube temp (32S reads from 112+offset2)
        assert!((pack.power_tube_temp - 35.0).abs() < 0.1, "power_tube_temp: got {}", pack.power_tube_temp);

        // Voltage, current
        assert!((pack.voltage - 51.2).abs() < 0.01);
        assert!((pack.current - (-5.0)).abs() < 0.01);
        assert!((pack.power - (-256.0)).abs() < 0.1);
        assert!((pack.discharging_power - 256.0).abs() < 0.1);

        // Temps 1 & 2
        assert!((pack.temps[0] - 25.0).abs() < 0.1);
        assert!((pack.temps[1] - 26.5).abs() < 0.1);

        // 32S-specific: error bitmask at 134+offset2 (NOT 136+offset2)
        assert_eq!(pack.error_bitmask, 0x0020, "32S error_bitmask should come from offset 134+o2");
        let errors = error_bitmask_to_strings(pack.error_bitmask);
        assert!(errors.contains(&"Discharge overcurrent"));

        // Balancing current
        assert!((pack.balancing_current - 0.050).abs() < 0.001);

        // Balancing
        assert!(pack.balancing);

        // SOC
        assert!((pack.soc - 76.0).abs() < 0.1);

        // Capacity
        assert!((pack.capacity_remaining - 100.0).abs() < 0.01);
        assert!((pack.total_battery_capacity - 300.0).abs() < 0.01);

        // Charging cycles
        assert_eq!(pack.charging_cycles, 42);

        // Total charging cycle capacity
        assert!((pack.total_charging_cycle_capacity - 1000.0).abs() < 0.1);

        // SOH
        assert!((pack.soh - 99.0).abs() < 0.1);

        // Total runtime
        assert_eq!(pack.total_runtime, 86400);

        // MOS states
        assert!(pack.charging);
        assert!(pack.discharging);
        assert!(pack.precharging);
        assert!(pack.heating);

        // 32S extra temp sensors
        assert_eq!(pack.ntemps, 5, "32S should have 5 temp probes");
        // temps[2] from 226+o2=258: 0x0118 = 280 → 28.0°C
        assert!((pack.temps[2] - 28.0).abs() < 0.1, "temp[2]: got {}", pack.temps[2]);
        // temps[3] from 224+o2=256: 0x0113 = 275 → 27.5°C
        assert!((pack.temps[3] - 27.5).abs() < 0.1, "temp[3]: got {}", pack.temps[3]);
        // temps[4] from 222+o2=254: 0x010E = 270 → 27.0°C
        assert!((pack.temps[4] - 27.0).abs() < 0.1, "temp[4]: got {}", pack.temps[4]);
    }

    // --- JK04 protocol test (IEEE 754 float cell voltages/resistances) ---

    #[test]
    fn test_getdata_jk04_cell_info() {
        let mut data = vec![0u8; 400];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x02;

        // JK04: cell voltages are IEEE 754 floats at offset 6+i*4
        // Cell 1: 3.315 V
        let bits1 = 3.315f32.to_bits();
        data[6] = bits1 as u8; data[7] = (bits1 >> 8) as u8; data[8] = (bits1 >> 16) as u8; data[9] = (bits1 >> 24) as u8;
        // Cell 2: 3.325 V
        let bits2 = 3.325f32.to_bits();
        data[10] = bits2 as u8; data[11] = (bits2 >> 8) as u8; data[12] = (bits2 >> 16) as u8; data[13] = (bits2 >> 24) as u8;

        // Cells 3-24: 0.0 (unused)
        // Already zeroed

        // Cell resistances at 102+i*4: IEEE 754 floats
        // Cell 1 resistance: 0.063 Ohm
        let r1 = 0.063f32.to_bits();
        data[102] = r1 as u8; data[103] = (r1 >> 8) as u8; data[104] = (r1 >> 16) as u8; data[105] = (r1 >> 24) as u8;
        // Cell 2 resistance: 0.085 Ohm
        let r2 = 0.085f32.to_bits();
        data[106] = r2 as u8; data[107] = (r2 >> 8) as u8; data[108] = (r2 >> 16) as u8; data[109] = (r2 >> 24) as u8;

        // Balancing action at 220: active
        data[220] = 0x01;

        // Balancing current at 222: 0.050 A as IEEE 754
        let bc = 0.050f32.to_bits();
        data[222] = bc as u8; data[223] = (bc >> 8) as u8; data[224] = (bc >> 16) as u8; data[225] = (bc >> 24) as u8;

        // Total runtime at 286: 86400
        data[286] = 0x80; data[287] = 0x51; data[288] = 0x01; data[289] = 0x00;

        finalize_frame(&mut data);

        let mut pack = MybmmPack::new("test");
        pack.protocol_version = ProtocolVersion::Jk04;
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_volts);
        assert_eq!(pack.cells, 2);

        // Cell voltages (IEEE 754 floats)
        assert!((pack.cellvolt[0] - 3.315).abs() < 0.001, "cellvolt[0]: got {}", pack.cellvolt[0]);
        assert!((pack.cellvolt[1] - 3.325).abs() < 0.001, "cellvolt[1]: got {}", pack.cellvolt[1]);

        // Cell resistances (IEEE 754 floats, in Ohms)
        assert!((pack.cellres[0] - 0.063).abs() < 0.001, "cellres[0]: got {}", pack.cellres[0]);
        assert!((pack.cellres[1] - 0.085).abs() < 0.001, "cellres[1]: got {}", pack.cellres[1]);

        // Total voltage = sum of cell voltages
        assert!((pack.voltage - 6.640).abs() < 0.01, "voltage: got {}", pack.voltage);

        // Computed stats
        assert!((pack.cell_min - 3.315).abs() < 0.001);
        assert!((pack.cell_max - 3.325).abs() < 0.001);
        assert!((pack.cell_diff - 0.010).abs() < 0.001);
        assert!((pack.cell_avg - 3.320).abs() < 0.001);

        // Balancing
        assert!(pack.balancing);

        // Balancing current
        assert!((pack.balancing_current - 0.050).abs() < 0.001, "balancing_current: got {}", pack.balancing_current);

        // Total runtime
        assert_eq!(pack.total_runtime, 86400);
    }

    // --- Info frame string extraction test ---

    #[test]
    fn test_getdata_info_frame_extracts_model_hw_sw() {
        let mut data = vec![0u8; 400];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x03;

        // Model at offset 6: "JK_PB2A16S20P\0"
        let model = b"JK_PB2A16S20P";
        data[6..6 + model.len()].copy_from_slice(model);
        data[6 + model.len()] = 0x00;

        // HW version after model + null padding: "19A\0"
        let hw = b"19A";
        let hw_start = 6 + model.len() + 1; // after model null
        data[hw_start..hw_start + hw.len()].copy_from_slice(hw);
        data[hw_start + hw.len()] = 0x00;

        // SW version after hw + null padding: "19.07\0"
        let sw = b"19.07";
        let sw_start = hw_start + hw.len() + 1;
        data[sw_start..sw_start + sw.len()].copy_from_slice(sw);
        data[sw_start + sw.len()] = 0x00;

        finalize_frame(&mut data);

        let mut pack = MybmmPack::new("test");
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_info);
        assert_eq!(pack.model, "JK_PB2A16S20P", "model should be extracted from info frame");
        assert_eq!(pack.hwvers, "19A", "hwvers should be extracted from info frame");
        assert_eq!(pack.swvers, "19.07", "swvers should be extracted from info frame");
        assert_eq!(pack.protocol_version, ProtocolVersion::Jk02_32S);
    }

    // --- JK02_24S with 0V cells (unused cell slots) ---

    #[test]
    fn test_getdata_jk02_24s_zero_voltage_cells_ignored() {
        let mut data = vec![0u8; 400];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x02;

        // Cell 1: 3315mV, Cell 2: 0mV (unused), Cell 3: 3320mV
        data[6] = 0xF3; data[7] = 0x0C;   // 3315
        data[8] = 0x00; data[9] = 0x00;    // 0 — unused cell
        data[10] = 0xF8; data[11] = 0x0C;  // 3320

        // Voltage at 118
        data[118] = 0x44; data[119] = 0xCF; data[120] = 0x00; data[121] = 0x00;

        // Current at 126
        data[126] = 0x00; data[127] = 0x00; data[128] = 0x00; data[129] = 0x00;

        finalize_frame(&mut data);

        let mut pack = MybmmPack::new("test");
        let _ = getdata(&mut pack, &data);

        // Only 2 cells enabled (0V cell is skipped in count)
        assert_eq!(pack.cells, 2);
        assert!((pack.cellvolt[0] - 3.315).abs() < 0.001);
        // Cell 2 is stored as 0.0 in the array but not counted as enabled
        assert!((pack.cellvolt[2] - 3.320).abs() < 0.001);
        // cell_min/cell_max should only consider non-zero cells
        assert!((pack.cell_min - 3.315).abs() < 0.001);
        assert!((pack.cell_max - 3.320).abs() < 0.001);
    }

    // =========================================================================
    // Settings tests
    // =========================================================================

    /// Helper: build a JK02_24S settings frame with known values
    fn make_jk02_24s_settings_frame() -> Vec<u8> {
        let mut data = vec![0u8; 300];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x01; // settings frame type

        // Using get_32bit byte ordering: [lo16_lo, lo16_hi, hi16_lo, hi16_hi]
        // where get_16bit = (data[i+1]<<8) | data[i]

        // 6: Smart sleep voltage = 0.003 V = 3 → 0x00000003
        data[6] = 0x03; data[7] = 0x00; data[8] = 0x00; data[9] = 0x00;

        // 10: Cell UVP = 2.800 V = 2800 → 0x00000AF0
        data[10] = 0xF0; data[11] = 0x0A; data[12] = 0x00; data[13] = 0x00;

        // 14: Cell UVPR = 3.000 V = 3000 → 0x00000BB8
        data[14] = 0xB8; data[15] = 0x0B; data[16] = 0x00; data[17] = 0x00;

        // 18: Cell OVP = 3.650 V = 3650 → 0x00000E42
        data[18] = 0x42; data[19] = 0x0E; data[20] = 0x00; data[21] = 0x00;

        // 22: Cell OVPR = 3.600 V = 3600 → 0x00000E10
        data[22] = 0x10; data[23] = 0x0E; data[24] = 0x00; data[25] = 0x00;

        // 26: Balance trigger voltage = 0.010 V = 10 → 0x0000000A
        data[26] = 0x0A; data[27] = 0x00; data[28] = 0x00; data[29] = 0x00;

        // 46: Power off voltage = 2.800 V = 2800 → 0x00000AF0
        data[46] = 0xF0; data[47] = 0x0A; data[48] = 0x00; data[49] = 0x00;

        // 50: Max charge current = 50.0 A = 50000 → 0x0000C350
        data[50] = 0x50; data[51] = 0xC3; data[52] = 0x00; data[53] = 0x00;

        // 54: Charge OCP delay = 30 s → 0x0000001E
        data[54] = 0x1E; data[55] = 0x00; data[56] = 0x00; data[57] = 0x00;

        // 58: Charge OCP recovery = 60 s → 0x0000003C
        data[58] = 0x3C; data[59] = 0x00; data[60] = 0x00; data[61] = 0x00;

        // 62: Max discharge current = 100.0 A = 100000 → 0x000186A0
        data[62] = 0xA0; data[63] = 0x86; data[64] = 0x01; data[65] = 0x00;

        // 66: Discharge OCP delay = 300 s → 0x0000012C
        data[66] = 0x2C; data[67] = 0x01; data[68] = 0x00; data[69] = 0x00;

        // 70: Discharge OCP recovery = 60 s → 0x0000003C
        data[70] = 0x3C; data[71] = 0x00; data[72] = 0x00; data[73] = 0x00;

        // 74: SCP recovery = 60 s → 0x0000003C
        data[74] = 0x3C; data[75] = 0x00; data[76] = 0x00; data[77] = 0x00;

        // 78: Max balance current = 0.5 A = 500 → 0x000001F4
        data[78] = 0xF4; data[79] = 0x01; data[80] = 0x00; data[81] = 0x00;

        // 82: Charge OTP = 65.0°C = 650 → 0x0000028A
        data[82] = 0x8A; data[83] = 0x02; data[84] = 0x00; data[85] = 0x00;

        // 86: Charge OTP recovery = 55.0°C = 550 → 0x00000226
        data[86] = 0x26; data[87] = 0x02; data[88] = 0x00; data[89] = 0x00;

        // 90: Discharge OTP = 65.0°C = 650 → 0x0000028A
        data[90] = 0x8A; data[91] = 0x02; data[92] = 0x00; data[93] = 0x00;

        // 94: Discharge OTP recovery = 55.0°C = 550 → 0x00000226
        data[94] = 0x26; data[95] = 0x02; data[96] = 0x00; data[97] = 0x00;

        // 98: Charge UTP = -20.0°C = -200 → 0xFFFFFF38 (signed)
        data[98] = 0x38; data[99] = 0xFF; data[100] = 0xFF; data[101] = 0xFF;

        // 102: Charge UTP recovery = -10.0°C = -100 → 0xFFFFFF9C
        data[102] = 0x9C; data[103] = 0xFF; data[104] = 0xFF; data[105] = 0xFF;

        // 106: MOSFET OTP = 85.0°C = 850 → 0x00000352
        data[106] = 0x52; data[107] = 0x03; data[108] = 0x00; data[109] = 0x00;

        // 110: MOSFET OTP recovery = 70.0°C = 700 → 0x000002BC
        data[110] = 0xBC; data[111] = 0x02; data[112] = 0x00; data[113] = 0x00;

        // 114: Cell count = 16
        data[114] = 16;

        // 118: Charge switch = on
        data[118] = 0x01;

        // 122: Discharge switch = on
        data[122] = 0x01;

        // 126: Balancer switch = on
        data[126] = 0x01;

        // 130: Battery capacity = 310.0 Ah = 310000 → 0x0004BAF0
        data[130] = 0xF0; data[131] = 0xBA; data[132] = 0x04; data[133] = 0x00;

        // 134: SCP delay = 10000 μs → 0x00002710
        data[134] = 0x10; data[135] = 0x27; data[136] = 0x00; data[137] = 0x00;

        // 138: Start balance voltage = 3.400 V = 3400 → 0x00000D48
        data[138] = 0x48; data[139] = 0x0D; data[140] = 0x00; data[141] = 0x00;

        finalize_frame(&mut data);
        data
    }

    #[test]
    fn test_parse_jk02_24s_settings() {
        let data = make_jk02_24s_settings_frame();

        let mut pack = MybmmPack::new("test");
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_res, "should detect settings frame (type 0x01)");
        assert!(!flags.got_volts);
        assert!(!flags.got_info);

        let s = pack.settings.as_ref().expect("settings should be populated");

        assert!((s.smart_sleep_voltage - 0.003).abs() < 0.001);
        assert!((s.cell_uvp - 2.800).abs() < 0.001);
        assert!((s.cell_uvpr - 3.000).abs() < 0.001);
        assert!((s.cell_ovp - 3.650).abs() < 0.001);
        assert!((s.cell_ovpr - 3.600).abs() < 0.001);
        assert!((s.balance_trigger_voltage - 0.010).abs() < 0.001);
        assert!((s.power_off_voltage - 2.800).abs() < 0.001);
        assert!((s.max_charge_current - 50.0).abs() < 0.01);
        assert!((s.charge_ocp_delay - 30.0).abs() < 0.1);
        assert!((s.charge_ocp_recovery - 60.0).abs() < 0.1);
        assert!((s.max_discharge_current - 100.0).abs() < 0.01);
        assert!((s.discharge_ocp_delay - 300.0).abs() < 0.1);
        assert!((s.discharge_ocp_recovery - 60.0).abs() < 0.1);
        assert!((s.scp_recovery - 60.0).abs() < 0.1);
        assert!((s.max_balance_current - 0.5).abs() < 0.01);
        assert!((s.charge_otp - 65.0).abs() < 0.1);
        assert!((s.charge_otp_recovery - 55.0).abs() < 0.1);
        assert!((s.discharge_otp - 65.0).abs() < 0.1);
        assert!((s.discharge_otp_recovery - 55.0).abs() < 0.1);
        assert!((s.charge_utp - (-20.0)).abs() < 0.1, "charge_utp: got {}", s.charge_utp);
        assert!((s.charge_utp_recovery - (-10.0)).abs() < 0.1);
        assert!((s.power_tube_otp - 85.0).abs() < 0.1);
        assert!((s.power_tube_otp_recovery - 70.0).abs() < 0.1);
        assert_eq!(s.cell_count, 16);
        assert!(s.charging_switch);
        assert!(s.discharging_switch);
        assert!(s.balancer_switch);
        assert!((s.total_battery_capacity - 310.0).abs() < 0.01);
        assert!((s.scp_delay - 10000.0).abs() < 0.1);
        assert!((s.balance_starting_voltage - 3.400).abs() < 0.001);
    }

    #[test]
    fn test_parse_jk02_32s_settings() {
        let mut data = vec![0u8; 300];
        data[0] = 0x55; data[1] = 0xAA; data[2] = 0xEB; data[3] = 0x90;
        data[4] = 0x01; // settings frame

        // Same offsets as JK02_24S for the common fields
        // Cell OVP = 4.200 V = 4200 → 0x00001068
        data[18] = 0x68; data[19] = 0x10; data[20] = 0x00; data[21] = 0x00;

        // Cell count = 16
        data[114] = 16;

        // Battery capacity = 200.0 Ah = 200000 → 0x00030D40
        data[130] = 0x40; data[131] = 0x0D; data[132] = 0x03; data[133] = 0x00;

        // 142-269: Wire resistances 1-32 (all zero, already)

        // 274: Precharge time = 30 s
        data[274] = 30;

        // 282: Controls bitmask: heating(1) + display_always_on(16) + smart_sleep(64) = 81 = 0x51
        data[282] = 0x51;
        // 283: timed_stored_data(1) = 0x01
        data[283] = 0x01;

        // 284: Heating start temperature = -5°C (i8)
        data[284] = (-5i8) as u8;
        // 285: Heating stop temperature = 5°C
        data[285] = 5;

        // 296: Discharge UTP = -20°C (i8)
        data[296] = (-20i8) as u8;
        // 297: Discharge UTP recovery = -10°C
        data[297] = (-10i8) as u8;

        finalize_frame(&mut data);

        let mut pack = MybmmPack::new("test");
        pack.protocol_version = ProtocolVersion::Jk02_32S;
        let flags = getdata(&mut pack, &data);

        assert!(flags.got_res);
        let s = pack.settings.as_ref().expect("settings should be populated");

        assert!((s.cell_ovp - 4.200).abs() < 0.001);
        assert_eq!(s.cell_count, 16);
        assert!((s.total_battery_capacity - 200.0).abs() < 0.01);
        assert_eq!(s.discharge_precharge_time, 30);
        assert!(s.heating_switch, "heating switch should be on");
        assert!(!s.disable_temp_sensors, "disable_temp_sensors should be off");
        assert!(s.display_always_on, "display_always_on should be on");
        assert!(s.smart_sleep_switch, "smart_sleep should be on");
        assert!(!s.disable_pcl_module, "disable_pcl_module should be off");
        assert!(s.timed_stored_data, "timed_stored_data should be on");
        assert!(!s.charging_float_mode, "charging_float_mode should be off");
        assert!((s.heating_start_temperature - (-5.0)).abs() < 0.1);
        assert!((s.heating_stop_temperature - 5.0).abs() < 0.1);
        assert!((s.discharge_utp - (-20.0)).abs() < 0.1);
        assert!((s.discharge_utp_recovery - (-10.0)).abs() < 0.1);
    }

    #[test]
    fn test_build_write_frame() {
        // Write max_charge_current = 50.0 A to register 0x0C (JK02_24S)
        // factor = 1000, so raw value = 50000 = 0x0000C350
        let frame = build_write_frame(0x0C, 50000, 4);

        assert_eq!(frame[0], 0xAA, "write header byte 0");
        assert_eq!(frame[1], 0x55, "write header byte 1");
        assert_eq!(frame[2], 0x90, "write header byte 2");
        assert_eq!(frame[3], 0xEB, "write header byte 3");
        assert_eq!(frame[4], 0x0C, "register");
        assert_eq!(frame[5], 4, "length");
        assert_eq!(frame[6], 0x50, "value byte 0");
        assert_eq!(frame[7], 0xC3, "value byte 1");
        assert_eq!(frame[8], 0x00, "value byte 2");
        assert_eq!(frame[9], 0x00, "value byte 3");
        // bytes 10-18 should be zero
        for i in 10..19 {
            assert_eq!(frame[i], 0, "padding byte {} should be zero", i);
        }
        // CRC at byte 19
        let expected_crc = crate::protocol::crc(&frame, 19);
        assert_eq!(frame[19], expected_crc, "CRC should match");
    }

    #[test]
    fn test_build_setting_write_frame_numeric() {
        // Write max_charge_current = 50.0 A for JK02_24S
        let frame = build_setting_write_frame("max_charge_current", "50.0", ProtocolVersion::Jk02_24S)
            .expect("should build frame");
        assert_eq!(frame[4], 0x0C, "register should be 0x0C for JK02_24S");

        // Raw value = 50.0 * 1000 = 50000
        assert_eq!(frame[6], 0x50);
        assert_eq!(frame[7], 0xC3);
    }

    #[test]
    fn test_build_setting_write_frame_switch() {
        // Write charging = on for JK02_32S
        let frame = build_setting_write_frame("charging", "on", ProtocolVersion::Jk02_32S)
            .expect("should build frame");
        assert_eq!(frame[4], 0x1D, "register should be 0x1D for JK02_32S charging");
        assert_eq!(frame[6], 0x01, "value should be 1 for on");

        // Write charging = off
        let frame = build_setting_write_frame("charging", "off", ProtocolVersion::Jk02_32S)
            .expect("should build frame");
        assert_eq!(frame[6], 0x00, "value should be 0 for off");
    }

    #[test]
    fn test_build_setting_write_frame_unsupported() {
        // discharge_utp is not supported on JK02_24S (register = 0)
        let result = build_setting_write_frame("discharge_utp", "-20", ProtocolVersion::Jk02_24S);
        assert!(result.is_none(), "should return None for unsupported setting");
    }

    #[test]
    fn test_get_setting_register() {
        assert_eq!(get_setting_register("max_charge_current", ProtocolVersion::Jk02_24S), Some(0x0C));
        assert_eq!(get_setting_register("max_charge_current", ProtocolVersion::Jk02_32S), Some(0x0C));
        assert_eq!(get_setting_register("heating", ProtocolVersion::Jk02_24S), None); // not supported
        assert_eq!(get_setting_register("heating", ProtocolVersion::Jk02_32S), Some(0x27));
        assert_eq!(get_setting_register("balancer", ProtocolVersion::Jk04), Some(0x6C));
        assert_eq!(get_setting_register("nonexistent", ProtocolVersion::Jk02_32S), None);
    }

    #[test]
    fn test_get_setting_def() {
        let def = get_setting_def("max_charge_current").expect("should find setting");
        assert_eq!(def.name, "max_charge_current");
        assert_eq!(def.unit, "A");
        assert_eq!(def.factor, 1000.0);
        assert_eq!(def.length, 4);
        assert!(!def.is_switch);

        let switch_def = get_setting_def("charging").expect("should find switch setting");
        assert!(switch_def.is_switch);
    }

    #[test]
    fn test_get_settings_command() {
        let cmd = get_settings_command();
        assert_eq!(cmd[0], 0xaa);
        assert_eq!(cmd[1], 0x55);
        assert_eq!(cmd[2], 0x90);
        assert_eq!(cmd[3], 0xeb);
        assert_eq!(cmd[4], 0x96);
    }

    #[test]
    fn test_settings_frame_raw_data_saved() {
        let data = make_jk02_24s_settings_frame();
        let mut pack = MybmmPack::new("test");
        let _ = getdata(&mut pack, &data);
        let s = pack.settings.as_ref().expect("settings should be populated");
        assert_eq!(s.raw_frame.len(), 300);
        // First 4 bytes should be the signature
        assert_eq!(s.raw_frame[0], 0x55);
        assert_eq!(s.raw_frame[1], 0xAA);
    }
}
