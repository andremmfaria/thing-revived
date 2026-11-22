//! Time-source host discovery. Parsers take `&str` so they are testable
//! without `ip` or a network.

use crate::utils::config::ClockConfig;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::process::Command;
use std::time::Duration;

pub const HOST_ENV: &str = "TERMINAL_AGENT_VERMELHO_HOST";

/// Discovers the time source host, highest precedence first:
/// 1. `TERMINAL_AGENT_VERMELHO_HOST` env var.
/// 2. `[clock] host` in the config file.
/// 3. The gateway of the device's own default route (`ip route show
///    default`), re-resolved every cycle so a lease change is picked up
///    without a restart.
/// 4. Fallback for the USB-CDC-NCM point-to-point link, which currently has
///    no default route at all, just a directly-connected /29 scope-link
///    subnet with no `via` gateway. Rather than hardcode the host's address
///    (explicitly disallowed), derive the subnet from the device's own
///    primary non-loopback IPv4 address + prefix and TCP-probe the ssh port
///    on each other host in that small range, returning the first that
///    answers. This re-resolves on every cycle, so it survives a lease
///    change just like the default-route path would.
pub fn discover_host(cfg: &ClockConfig) -> Option<String> {
    let env = std::env::var(HOST_ENV).ok();
    if let Some(h) = pick_explicit(env.as_deref(), cfg.host.as_deref()) {
        return Some(h);
    }
    if let Some(h) = default_route_gateway() {
        return Some(h);
    }
    subnet_probe_fallback(cfg)
}

/// Precedence of the two explicit sources; empty values count as unset.
pub fn pick_explicit(env: Option<&str>, configured: Option<&str>) -> Option<String> {
    [env, configured]
        .into_iter()
        .flatten()
        .find(|h| !h.is_empty())
        .map(str::to_string)
}

fn default_route_gateway() -> Option<String> {
    let out = Command::new("ip").args(["route", "show", "default"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_default_gateway(&String::from_utf8_lossy(&out.stdout))
}

/// Extracts the gateway from `ip route show default` output, e.g.
/// `default via 10.42.1.30 dev uncm0 ...`.
pub fn parse_default_gateway(text: &str) -> Option<String> {
    let mut parts = text.split_whitespace();
    while let Some(tok) = parts.next() {
        if tok == "via" {
            return parts.next().map(|s| s.to_string());
        }
    }
    None
}

/// Every other usable host (not network, broadcast or `own`) of the subnet
/// `own/prefix`, in ascending order. Empty for prefixes above /30 and
/// invalid prefixes.
pub fn subnet_hosts(own: Ipv4Addr, prefix: u32) -> Vec<Ipv4Addr> {
    if prefix > 32 {
        return Vec::new();
    }
    let own_u32 = u32::from(own);
    let mask: u32 = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
    let network = own_u32 & mask;
    let host_count = 1u64 << (32 - prefix);
    (1..host_count.saturating_sub(1))
        .map(|i| network | i as u32)
        .filter(|c| *c != own_u32)
        .map(Ipv4Addr::from)
        .collect()
}

/// Parses `ip -4 -o addr show` output into the ssh probe candidates:
/// non-loopback interfaces whose prefix is at least `min_prefix` (so only
/// small subnets are probed, never an accidental port scan of a big LAN).
pub fn probe_candidates(text: &str, min_prefix: u32) -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.contains(" lo ") || line.trim_start().starts_with("1:") {
            continue; // skip loopback
        }
        let cidr = match line.split_whitespace().find(|p| p.contains('/')) {
            Some(c) => c,
            None => continue,
        };
        let Some((addr_str, prefix_str)) = cidr.split_once('/') else { continue };
        let (Ok(own), Ok(prefix)) = (addr_str.parse::<Ipv4Addr>(), prefix_str.parse::<u32>()) else {
            continue;
        };
        if prefix < min_prefix {
            continue; // subnet too large to probe cheaply
        }
        out.extend(subnet_hosts(own, prefix));
    }
    out
}

fn subnet_probe_fallback(cfg: &ClockConfig) -> Option<String> {
    let out = Command::new("ip").args(["-4", "-o", "addr", "show"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let timeout = Duration::from_millis(cfg.probe_timeout_ms);
    probe_candidates(&text, cfg.min_probe_prefix)
        .into_iter()
        .find(|c| TcpStream::connect_timeout(&SocketAddr::new((*c).into(), cfg.ssh_port), timeout).is_ok())
        .map(|c| c.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_precedence_env_then_config() {
        assert_eq!(pick_explicit(Some("e"), Some("c")).as_deref(), Some("e"));
        assert_eq!(pick_explicit(Some(""), Some("c")).as_deref(), Some("c"));
        assert_eq!(pick_explicit(None, Some("c")).as_deref(), Some("c"));
        assert_eq!(pick_explicit(None, Some("")), None);
        assert_eq!(pick_explicit(None, None), None);
    }

    #[test]
    fn gateway_parsing() {
        assert_eq!(
            parse_default_gateway("default via 10.42.1.30 dev uncm0 proto dhcp metric 100\n").as_deref(),
            Some("10.42.1.30")
        );
        assert_eq!(parse_default_gateway(""), None);
        assert_eq!(parse_default_gateway("10.42.1.0/29 dev uncm0 scope link\n"), None);
        assert_eq!(parse_default_gateway("default via"), None);
    }

    #[test]
    fn hosts_of_a_slash_29() {
        let own: Ipv4Addr = "10.42.1.2".parse().unwrap();
        let hosts: Vec<String> = subnet_hosts(own, 29).iter().map(|h| h.to_string()).collect();
        assert_eq!(hosts, ["10.42.1.1", "10.42.1.3", "10.42.1.4", "10.42.1.5", "10.42.1.6"]);
    }

    #[test]
    fn hosts_edge_prefixes() {
        let own: Ipv4Addr = "10.0.0.1".parse().unwrap();
        assert!(subnet_hosts(own, 31).is_empty());
        assert!(subnet_hosts(own, 32).is_empty());
        assert!(subnet_hosts(own, 33).is_empty());
        assert_eq!(subnet_hosts(own, 30).len(), 1); // .2 only, .1 is own
        assert_eq!(subnet_hosts(own, 28).len(), 13);
    }

    const IP_ADDR: &str = "1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever\n\
2: eth0    inet 192.168.1.20/24 brd 192.168.1.255 scope global eth0\n\
3: uncm0    inet 10.42.1.2/29 brd 10.42.1.7 scope global uncm0\n";

    #[test]
    fn candidates_skip_loopback_and_large_subnets() {
        let c: Vec<String> = probe_candidates(IP_ADDR, 28).iter().map(|h| h.to_string()).collect();
        assert_eq!(c, ["10.42.1.1", "10.42.1.3", "10.42.1.4", "10.42.1.5", "10.42.1.6"]);
        // A looser limit also admits the /24.
        assert_eq!(probe_candidates(IP_ADDR, 24).len(), 253 + 5);
    }

    #[test]
    fn candidates_tolerate_garbage_lines() {
        assert!(probe_candidates("", 28).is_empty());
        assert!(probe_candidates("3: x inet notanip/29\n4: y inet 10.0.0.1/xx\n5: z nothing here\n", 28).is_empty());
        // A bad line does not hide a good one after it.
        let t = "3: x inet bad/29\n4: y inet 10.0.0.1/30 scope global y\n";
        assert_eq!(probe_candidates(t, 28), ["10.0.0.2".parse::<Ipv4Addr>().unwrap()]);
    }
}
