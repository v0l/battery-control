use bitflags::bitflags;

bitflags! {
    /// What a backend can do. Read capabilities describe which parts of
    /// [`crate::BatteryStatus`] are populated; control capabilities gate
    /// [`crate::Command`] variants.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Capabilities: u32 {
        // --- read ---
        /// Reports SOC / voltage / current basics.
        const READ_BASIC        = 1 << 0;
        /// Reports per-cell voltages.
        const READ_CELLS        = 1 << 1;
        /// Reports output port states/power.
        const READ_PORTS        = 1 << 2;
        /// Reports temperature.
        const READ_TEMPERATURE  = 1 << 3;
        /// Reports BMS charge/discharge current limits.
        const READ_LIMITS       = 1 << 4;
        /// Reports alarm/warning flags.
        const READ_ALARMS       = 1 << 5;

        // --- control ---
        // (Port controllability is per-port; see `PortInfo::settable`.)
        /// Can toggle the charge MOSFET.
        const TOGGLE_CHARGE     = 1 << 17;
        /// Can toggle the discharge MOSFET.
        const TOGGLE_DISCHARGE  = 1 << 18;
        /// Can toggle the balancer.
        const TOGGLE_BALANCER   = 1 << 19;
        /// Can set a charge-limit percentage.
        const SET_CHARGE_LIMIT  = 1 << 20;
        /// Can write named settings.
        const WRITE_SETTINGS    = 1 << 21;
        /// Requires an authentication / binding step (see
        /// [`Battery::authenticate`](crate::Battery::authenticate)) before
        /// control works.
        const REQUIRES_AUTH     = 1 << 22;
    }
}

impl Capabilities {
    /// True if this backend supports any control command (i.e. is not read-only).
    pub fn is_controllable(&self) -> bool {
        self.intersects(
            Capabilities::TOGGLE_CHARGE
                | Capabilities::TOGGLE_DISCHARGE
                | Capabilities::TOGGLE_BALANCER
                | Capabilities::SET_CHARGE_LIMIT
                | Capabilities::WRITE_SETTINGS,
        )
    }
}
