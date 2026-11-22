//! Maps device-module data (`wifi`, `bluetooth`) onto wire events. This is
//! the one place where the D-Bus and wpa_supplicant data models meet the
//! serde-only `protocol` types.

use crate::domain::bluetooth::PairedDevice;
use crate::protocol::{BridgeEvent, PairedDeviceWire, WifiNetworkWire, WifiSavedWire};
use crate::wifi;

impl From<wifi::WifiEvent> for BridgeEvent {
    fn from(e: wifi::WifiEvent) -> Self {
        use wifi::WifiEvent as W;
        match e {
            W::Available { available, interface } => BridgeEvent::WifiAvailable { available, interface },
            W::Status(s) => BridgeEvent::WifiStatus {
                state: s.state.wire().into(),
                ssid: s.ssid,
                ip: s.ip,
                interface: s.interface,
                signal_pct: s.signal_pct,
                ethernet_up: s.ethernet_up,
            },
            W::ScanStarted => BridgeEvent::WifiScanStarted {},
            W::ScanResults(list) => BridgeEvent::WifiScanResults {
                networks: list
                    .into_iter()
                    .map(|n| WifiNetworkWire {
                        ssid: n.ssid,
                        signal_pct: n.signal_pct,
                        signal_dbm: n.signal_dbm,
                        security: n.security.wire().into(),
                        supported: n.security.supported(),
                    })
                    .collect(),
            },
            W::Connecting { ssid } => BridgeEvent::WifiConnecting { ssid },
            W::ConnectResult { ssid, error } => BridgeEvent::WifiConnectResult {
                ssid,
                success: error.is_none(),
                message: error.as_ref().map(|e| e.message()),
                error: error.map(|e| e.code().to_string()),
            },
            W::SavedList(list) => BridgeEvent::WifiSavedList {
                networks: list.into_iter().map(|n| WifiSavedWire { ssid: n.ssid, current: n.current }).collect(),
            },
            W::Error { context, message } => BridgeEvent::WifiError { context, message },
        }
    }
}

impl From<PairedDevice> for PairedDeviceWire {
    fn from(d: PairedDevice) -> Self {
        PairedDeviceWire { address: d.address, name: d.name, connected: d.connected }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_result_event_wire_shape() {
        let ok = BridgeEvent::from(wifi::WifiEvent::ConnectResult { ssid: "Home".into(), error: None });
        let v: serde_json::Value = serde_json::to_value(&ok).unwrap();
        assert_eq!(v["type"], "wifi_connect_result");
        assert_eq!(v["success"], true);
        assert!(v["error"].is_null());

        let bad = BridgeEvent::from(wifi::WifiEvent::ConnectResult {
            ssid: "Home".into(),
            error: Some(wifi::FailKind::WrongPassword),
        });
        let v: serde_json::Value = serde_json::to_value(&bad).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["error"], "wrong_password");
        assert_eq!(v["message"], "Wrong password");
    }

    #[test]
    fn status_event_wire_shape() {
        let e = BridgeEvent::from(wifi::WifiEvent::Status(wifi::StatusInfo {
            state: wifi::ConnState::Connected,
            ssid: Some("Home".into()),
            ip: Some("192.168.1.9".into()),
            interface: "wlan0".into(),
            signal_pct: Some(80),
            ethernet_up: true,
        }));
        let v: serde_json::Value = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "wifi_status");
        assert_eq!(v["state"], "connected");
        assert_eq!(v["ethernet_up"], true);
        assert_eq!(v["signal_pct"], 80);
    }
}
