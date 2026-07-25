//! Device status payloads exchanged on the transport-keepalive channel.
//!
//! Each side reports a 2-byte status tail after the 8-byte ping/pong
//! nonce, so the UIs on both sides can render real-time battery /
//! device-class info without a separate frame type.
//!
//! Wire layout (10-byte total ping/pong payload):
//!   | 0  | 8 | random nonce                      |
//!   | 8  | 1 | battery %  (0..=100, 0xFF unknown)|
//!   | 9  | 1 | device_class                      |

use std::fs;
use std::path::PathBuf;

/// Device-class enum carried as a single byte in the keepalive tail.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    Unknown = 0,
    Laptop = 1,
    Phone = 2,
    Tablet = 3,
    Earbuds = 4,
}

impl DeviceClass {
    pub fn from_byte(b: u8) -> Self {
        match b {
            1 => DeviceClass::Laptop,
            2 => DeviceClass::Phone,
            3 => DeviceClass::Tablet,
            4 => DeviceClass::Earbuds,
            _ => DeviceClass::Unknown,
        }
    }
}

/// Battery percentage or `None` if the device can't report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryPct(pub Option<u8>);

impl BatteryPct {
    pub fn to_byte(self) -> u8 {
        self.0.unwrap_or(0xFF)
    }
    pub fn from_byte(b: u8) -> Self {
        if b == 0xFF || b > 100 {
            BatteryPct(None)
        } else {
            BatteryPct(Some(b))
        }
    }
}

/// Local device's current status — what we'll embed in our outgoing
/// keepalive frames.
#[derive(Debug, Clone, Copy)]
pub struct LocalStatus {
    pub battery: BatteryPct,
    pub class: DeviceClass,
}

impl LocalStatus {
    /// Append the 2-byte status tail to a vec already containing the
    /// 8-byte ping/pong nonce.
    pub fn append_to(&self, out: &mut Vec<u8>) {
        out.push(self.battery.to_byte());
        out.push(self.class as u8);
    }
}

/// Read the laptop's battery percentage from `/sys/class/power_supply`.
/// Returns the first `BAT*/capacity` value found. None if no battery
/// (desktop) or read failure.
pub fn read_local_battery() -> BatteryPct {
    let dir = match fs::read_dir("/sys/class/power_supply") {
        Ok(d) => d,
        Err(_) => return BatteryPct(None),
    };
    for entry in dir.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("BAT") {
            continue;
        }
        let mut path: PathBuf = entry.path();
        path.push("capacity");
        if let Ok(s) = fs::read_to_string(&path) {
            if let Ok(v) = s.trim().parse::<u8>() {
                if v <= 100 {
                    return BatteryPct(Some(v));
                }
            }
        }
    }
    BatteryPct(None)
}

/// Whether the laptop is currently plugged in. PRIMARY signal is the AC
/// adapter (`type=Mains`, `online=1`) — plugged-in is true even when the
/// battery reports "Not charging" (topped off / under a charge-limit
/// threshold), which is the common desktop/laptop case. Falls back to the
/// battery `status` ("Charging"/"Full") when there's no Mains supply.
pub fn read_local_charging() -> bool {
    let dir = match fs::read_dir("/sys/class/power_supply") {
        Ok(d) => d,
        Err(_) => return false,
    };
    let mut bat_charging = false;
    for entry in dir.flatten() {
        let path: PathBuf = entry.path();
        // AC adapter / Mains online=1 → plugged in, regardless of the battery
        // actively charging (it may sit at "Not charging" near full).
        let ty = fs::read_to_string(path.join("type")).unwrap_or_default();
        if ty.trim().eq_ignore_ascii_case("Mains") {
            if fs::read_to_string(path.join("online"))
                .map(|o| o.trim() == "1")
                .unwrap_or(false)
            {
                return true;
            }
            continue;
        }
        // Battery status fallback (machines without a Mains supply node).
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("BAT") {
            if let Ok(s) = fs::read_to_string(path.join("status")) {
                let s = s.trim();
                if s.eq_ignore_ascii_case("Charging") || s.eq_ignore_ascii_case("Full") {
                    bat_charging = true;
                }
            }
        }
    }
    bat_charging
}

/// Helper for callers that just want to build the local-side payload
/// quickly.
pub fn local_laptop_status() -> LocalStatus {
    LocalStatus {
        battery: read_local_battery(),
        class: DeviceClass::Laptop,
    }
}

/// Decode the 2-byte status tail of a peer's keepalive payload.
/// Returns `None` if the payload is shorter than 10 bytes (legacy
/// peer that doesn't yet carry status).
pub fn decode_peer_status(payload: &[u8]) -> Option<(BatteryPct, DeviceClass)> {
    if payload.len() < 10 {
        return None;
    }
    let battery = BatteryPct::from_byte(payload[8]);
    let class = DeviceClass::from_byte(payload[9]);
    Some((battery, class))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_roundtrip() {
        for v in [0, 1, 50, 99, 100] {
            assert_eq!(BatteryPct(Some(v)).to_byte(), v);
            assert_eq!(BatteryPct::from_byte(v), BatteryPct(Some(v)));
        }
        assert_eq!(BatteryPct(None).to_byte(), 0xFF);
        assert_eq!(BatteryPct::from_byte(0xFF), BatteryPct(None));
        assert_eq!(BatteryPct::from_byte(101), BatteryPct(None));
    }

    #[test]
    fn class_byte() {
        assert_eq!(DeviceClass::from_byte(1), DeviceClass::Laptop);
        assert_eq!(DeviceClass::from_byte(2), DeviceClass::Phone);
        assert_eq!(DeviceClass::from_byte(99), DeviceClass::Unknown);
    }

    #[test]
    fn decode_short_payload_returns_none() {
        assert!(decode_peer_status(&[0u8; 8]).is_none());
        assert!(decode_peer_status(&[0u8; 9]).is_none());
    }

    #[test]
    fn decode_full_payload() {
        let mut p = vec![0u8; 8];
        p.push(75);
        p.push(2);
        let (bat, cls) = decode_peer_status(&p).unwrap();
        assert_eq!(bat, BatteryPct(Some(75)));
        assert_eq!(cls, DeviceClass::Phone);
    }
}
