//! Pure paired-device selection: filter, reduce and order the facts read from
//! BlueZ's object dump. The adapter turns the D-Bus dump into `RawDevice`s.

use crate::domain::bluetooth::PairedDevice;

/// One `org.bluez.Device1` object reduced to the properties this crate reads.
/// `address` is optional because BlueZ may omit it; such entries are skipped.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RawDevice {
    pub address: Option<String>,
    pub name: Option<String>,
    pub paired: bool,
    pub connected: bool,
}

/// Keeps every `Paired` device that has an address, reduced to the three
/// fields the paired-devices screen needs. Sorted by address for a stable,
/// UI-friendly order (BlueZ's own `GetManagedObjects` has no defined order).
pub fn extract_paired_devices(devices: &[RawDevice]) -> Vec<PairedDevice> {
    let mut out: Vec<PairedDevice> = devices
        .iter()
        .filter(|d| d.paired)
        .filter_map(|d| {
            Some(PairedDevice { address: d.address.clone()?, name: d.name.clone(), connected: d.connected })
        })
        .collect();
    out.sort_by(|a, b| a.address.cmp(&b.address));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(address: &str, name: Option<&str>, paired: bool, connected: bool) -> RawDevice {
        RawDevice { address: Some(address.into()), name: name.map(Into::into), paired, connected }
    }

    #[test]
    fn filters_out_unpaired_devices() {
        let d = extract_paired_devices(&[
            dev("AA:AA:AA:AA:AA:AA", Some("Keyboard"), true, true),
            dev("BB:BB:BB:BB:BB:BB", Some("Scanned, not paired"), false, false),
        ]);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].address, "AA:AA:AA:AA:AA:AA");
    }

    #[test]
    fn reports_name_and_connected_state() {
        let d = extract_paired_devices(&[
            dev("AA:AA:AA:AA:AA:AA", Some("Keyboard"), true, true),
            dev("CC:CC:CC:CC:CC:CC", None, true, false),
        ]);
        assert_eq!(d.len(), 2);
        let kb = d.iter().find(|d| d.address == "AA:AA:AA:AA:AA:AA").unwrap();
        assert_eq!(kb.name.as_deref(), Some("Keyboard"));
        assert!(kb.connected);
        let unnamed = d.iter().find(|d| d.address == "CC:CC:CC:CC:CC:CC").unwrap();
        assert_eq!(unnamed.name, None);
        assert!(!unnamed.connected);
    }

    #[test]
    fn sorts_by_address() {
        let d = extract_paired_devices(&[
            dev("FF:FF:FF:FF:FF:FF", None, true, false),
            dev("11:11:11:11:11:11", None, true, false),
        ]);
        assert_eq!(d[0].address, "11:11:11:11:11:11");
        assert_eq!(d[1].address, "FF:FF:FF:FF:FF:FF");
    }

    #[test]
    fn skips_entries_without_an_address() {
        let d = extract_paired_devices(&[RawDevice { paired: true, ..RawDevice::default() }]);
        assert!(d.is_empty());
    }
}
