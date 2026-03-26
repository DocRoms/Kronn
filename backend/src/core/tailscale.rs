//! Network & VPN auto-detection.
//!
//! Detects Tailscale, VPN interfaces, and LAN IPs. Used by `advertised_host()`
//! to generate invite codes that work across network changes.

use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Cached Tailscale detection result with TTL.
struct TailscaleCache {
    ip: Option<String>,
    checked_at: Instant,
}

static CACHE: OnceLock<Mutex<Option<TailscaleCache>>> = OnceLock::new();

/// Cache TTL: re-check every 60 seconds.
const CACHE_TTL: Duration = Duration::from_secs(60);

/// Detect the Tailscale IPv4 address, if available.
///
/// Tries two methods in order:
/// 1. `tailscale ip -4` command (most reliable)
/// 2. Scan network interfaces for 100.x.x.x addresses (fallback)
///
/// Results are cached for 60 seconds to avoid repeated subprocess calls.
pub async fn detect_ip() -> Option<String> {
    let cache_mutex = CACHE.get_or_init(|| Mutex::new(None));
    let mut cache = cache_mutex.lock().await;

    // Return cached result if still valid
    if let Some(ref c) = *cache {
        if c.checked_at.elapsed() < CACHE_TTL {
            return c.ip.clone();
        }
    }

    // Try host env var first (Docker scenario), then CLI, then interface scan
    let ip = detect_via_host_env()
        .or(detect_via_cli().await)
        .or_else(detect_via_interface);

    *cache = Some(TailscaleCache {
        ip: ip.clone(),
        checked_at: Instant::now(),
    });

    ip
}

/// Extract Tailscale IP from `KRONN_HOST_IPS` env var (Docker scenario).
fn detect_via_host_env() -> Option<String> {
    let val = std::env::var("KRONN_HOST_IPS").ok()?;
    for entry in val.split(',') {
        let parts: Vec<&str> = entry.splitn(2, ':').collect();
        if parts.len() == 2 && is_tailscale_ip(parts[1]) {
            return Some(parts[1].to_string());
        }
    }
    None
}

/// Try `tailscale ip -4` command.
async fn detect_via_cli() -> Option<String> {
    let output = tokio::process::Command::new("tailscale")
        .args(["ip", "-4"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let ip = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if is_tailscale_ip(&ip) {
        Some(ip)
    } else {
        None
    }
}

/// Scan network interfaces for a Tailscale IP (100.x.x.x range).
fn detect_via_interface() -> Option<String> {
    // Read /proc/net/fib_trie or use a simpler approach: parse `ip addr`
    // For portability, just check if any 100.x.x.x exists in /proc/net/fib_trie
    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/net/fib_trie") {
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("100.") && is_tailscale_ip(trimmed) {
                    // Verify it's a valid IP (not a subnet)
                    if trimmed.parse::<std::net::Ipv4Addr>().is_ok() {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }
    }

    // macOS/other: try parsing ifconfig output synchronously
    #[cfg(not(target_os = "linux"))]
    {
        if let Ok(output) = std::process::Command::new("ifconfig").output() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                let trimmed = line.trim();
                // Look for "inet 100.x.x.x" pattern
                if let Some(rest) = trimmed.strip_prefix("inet ") {
                    let ip = rest.split_whitespace().next().unwrap_or("");
                    if is_tailscale_ip(ip) {
                        return Some(ip.to_string());
                    }
                }
            }
        }
    }

    None
}

/// Check if an IP is in the Tailscale CGNAT range (100.64.0.0/10).
fn is_tailscale_ip(ip: &str) -> bool {
    let Ok(addr) = ip.parse::<std::net::Ipv4Addr>() else {
        return false;
    };
    let octets = addr.octets();
    // 100.64.0.0/10 = first octet 100, second octet 64-127
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

/// Synchronous version for use in non-async contexts (e.g., tests).
/// Only uses interface scanning, not CLI.
pub fn detect_ip_sync() -> Option<String> {
    detect_via_interface()
}

// ─── Detected network interface ─────────────────────────────────────────────

/// A detected network IP with its type.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct DetectedIp {
    pub ip: String,
    /// "tailscale", "vpn", or "lan"
    pub kind: String,
    pub label: String,
}

/// Detect all usable IPv4 addresses on this machine.
/// Returns IPs grouped by kind: tailscale > vpn > lan.
///
/// When running inside Docker, the container only sees its own interfaces.
/// The `KRONN_HOST_IPS` env var (set by Makefile at startup) provides the
/// host's real IPs in format: `iface:ip,iface:ip,...`
pub async fn detect_all_ips() -> Vec<DetectedIp> {
    // Prefer host IPs from env (Docker scenario)
    if let Some(host_ips) = parse_host_ips_env() {
        if !host_ips.is_empty() {
            return host_ips;
        }
    }

    let mut ips = Vec::new();

    // Tailscale (via CLI first, then interface scan)
    if let Some(ts_ip) = detect_ip().await {
        ips.push(DetectedIp {
            ip: ts_ip,
            kind: "tailscale".into(),
            label: "Tailscale VPN".into(),
        });
    }

    // Scan all interfaces for VPN and LAN IPs
    for (ip, kind, label) in scan_all_interfaces() {
        // Skip if already added as Tailscale
        if ips.iter().any(|d| d.ip == ip) {
            continue;
        }
        ips.push(DetectedIp { ip, kind, label });
    }

    ips
}

/// Parse `KRONN_HOST_IPS` env var: `iface:ip,iface:ip,...`
/// Each entry is classified by `classify_ip`.
fn parse_host_ips_env() -> Option<Vec<DetectedIp>> {
    let val = std::env::var("KRONN_HOST_IPS").ok()?;
    if val.is_empty() {
        return None;
    }
    let mut ips = Vec::new();
    for entry in val.split(',') {
        let parts: Vec<&str> = entry.splitn(2, ':').collect();
        if parts.len() != 2 {
            continue;
        }
        let iface = parts[0];
        let ip = parts[1];
        if let Some((ip_str, kind, label)) = classify_ip(ip, iface) {
            ips.push(DetectedIp { ip: ip_str, kind, label });
        }
    }
    Some(ips)
}

/// Scan network interfaces and classify IPs.
fn scan_all_interfaces() -> Vec<(String, String, String)> {
    let mut results = Vec::new();

    #[cfg(target_os = "linux")]
    {
        // Parse `ip -4 addr show` output
        if let Ok(output) = std::process::Command::new("ip")
            .args(["-4", "addr", "show"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut current_iface = String::new();
            for line in text.lines() {
                // Interface line: "2: eth0: <...>"
                if !line.starts_with(' ') {
                    if let Some(name) = line.split(':').nth(1) {
                        current_iface = name.trim().to_string();
                    }
                }
                // IP line: "    inet 10.0.0.5/24 ..."
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("inet ") {
                    let ip = rest.split('/').next().unwrap_or("").trim();
                    if let Some(classified) = classify_ip(ip, &current_iface) {
                        results.push(classified);
                    }
                }
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        if let Ok(output) = std::process::Command::new("ifconfig").output() {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut current_iface = String::new();
            for line in text.lines() {
                // Interface line: "en0: flags=..."
                if !line.starts_with('\t') && !line.starts_with(' ') {
                    if let Some(name) = line.split(':').next() {
                        current_iface = name.to_string();
                    }
                }
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("inet ") {
                    let ip = rest.split_whitespace().next().unwrap_or("");
                    if let Some(classified) = classify_ip(ip, &current_iface) {
                        results.push(classified);
                    }
                }
            }
        }
    }

    results
}

/// Classify an IP address as tailscale, vpn, or lan. Returns None for localhost/docker.
fn classify_ip(ip: &str, iface: &str) -> Option<(String, String, String)> {
    let Ok(addr) = ip.parse::<std::net::Ipv4Addr>() else {
        return None;
    };
    let octets = addr.octets();

    // Skip localhost
    if octets[0] == 127 {
        return None;
    }

    // Skip Docker bridge (172.17.x.x typically)
    if iface.starts_with("docker") || iface.starts_with("br-") || iface.starts_with("veth") {
        return None;
    }

    // Tailscale: 100.64.0.0/10
    if octets[0] == 100 && (64..=127).contains(&octets[1]) {
        return Some((ip.into(), "tailscale".into(), "Tailscale VPN".into()));
    }

    // VPN interfaces: tun*, utun*, tap*, wg* (WireGuard), ppp*
    let is_vpn_iface = iface.starts_with("tun")
        || iface.starts_with("utun")
        || iface.starts_with("tap")
        || iface.starts_with("wg")
        || iface.starts_with("ppp")
        || iface.starts_with("tailscale");

    // Private ranges on VPN interfaces → VPN
    if is_vpn_iface {
        return Some((ip.into(), "vpn".into(), format!("VPN ({})", iface)));
    }

    // Private LAN ranges: 10.x.x.x, 172.16-31.x.x, 192.168.x.x
    let is_private = octets[0] == 10
        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 168);

    if is_private {
        return Some((ip.into(), "lan".into(), format!("LAN ({})", iface)));
    }

    // Public IP — still useful
    Some((ip.into(), "lan".into(), format!("Public ({})", iface)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_tailscale_ip_valid() {
        assert!(is_tailscale_ip("100.64.0.1"));
        assert!(is_tailscale_ip("100.100.50.25"));
        assert!(is_tailscale_ip("100.127.255.254"));
    }

    #[test]
    fn is_tailscale_ip_invalid() {
        assert!(!is_tailscale_ip("192.168.1.1"));
        assert!(!is_tailscale_ip("10.0.0.1"));
        assert!(!is_tailscale_ip("100.128.0.1")); // outside /10 range
        assert!(!is_tailscale_ip("100.63.255.255")); // below range
        assert!(!is_tailscale_ip("not-an-ip"));
        assert!(!is_tailscale_ip(""));
    }

    #[tokio::test]
    async fn detect_ip_returns_cached_result() {
        // First call populates cache
        let ip1 = detect_ip().await;
        // Second call should return same result (from cache)
        let ip2 = detect_ip().await;
        assert_eq!(ip1, ip2);
    }

    #[test]
    fn classify_ip_localhost_skipped() {
        assert!(classify_ip("127.0.0.1", "lo").is_none());
    }

    #[test]
    fn classify_ip_docker_skipped() {
        assert!(classify_ip("172.17.0.1", "docker0").is_none());
        assert!(classify_ip("172.18.0.1", "br-abc123").is_none());
    }

    #[test]
    fn classify_ip_tailscale() {
        let (ip, kind, _) = classify_ip("100.100.50.1", "tailscale0").unwrap();
        assert_eq!(ip, "100.100.50.1");
        assert_eq!(kind, "tailscale");
    }

    #[test]
    fn classify_ip_vpn_tun() {
        let (ip, kind, label) = classify_ip("10.8.0.5", "tun0").unwrap();
        assert_eq!(ip, "10.8.0.5");
        assert_eq!(kind, "vpn");
        assert!(label.contains("tun0"));
    }

    #[test]
    fn classify_ip_lan() {
        let (ip, kind, _) = classify_ip("192.168.1.50", "eth0").unwrap();
        assert_eq!(ip, "192.168.1.50");
        assert_eq!(kind, "lan");
    }

    #[test]
    fn classify_ip_private_10_on_regular_iface() {
        let (_, kind, _) = classify_ip("10.0.0.5", "eth0").unwrap();
        assert_eq!(kind, "lan");
    }

    #[test]
    fn classify_ip_private_10_on_vpn_iface() {
        let (_, kind, _) = classify_ip("10.0.0.5", "tun0").unwrap();
        assert_eq!(kind, "vpn");
    }

    #[test]
    fn parse_host_ips_env_valid() {
        std::env::set_var("KRONN_HOST_IPS", "eth0:192.168.1.50,tailscale0:100.100.50.1,tun0:10.8.0.5");
        let ips = parse_host_ips_env().unwrap();
        assert_eq!(ips.len(), 3);
        assert_eq!(ips[0].ip, "192.168.1.50");
        assert_eq!(ips[0].kind, "lan");
        assert_eq!(ips[1].ip, "100.100.50.1");
        assert_eq!(ips[1].kind, "tailscale");
        assert_eq!(ips[2].ip, "10.8.0.5");
        assert_eq!(ips[2].kind, "vpn");
        std::env::remove_var("KRONN_HOST_IPS");
    }

    #[test]
    fn parse_host_ips_env_empty() {
        std::env::set_var("KRONN_HOST_IPS", "");
        assert!(parse_host_ips_env().is_none());
        std::env::remove_var("KRONN_HOST_IPS");
    }

    #[test]
    fn parse_host_ips_env_unset() {
        std::env::remove_var("KRONN_HOST_IPS");
        assert!(parse_host_ips_env().is_none());
    }

    #[test]
    fn parse_host_ips_env_skips_localhost_and_docker() {
        std::env::set_var("KRONN_HOST_IPS", "lo:127.0.0.1,docker0:172.17.0.1,eth0:192.168.1.10");
        let ips = parse_host_ips_env().unwrap();
        assert_eq!(ips.len(), 1);
        assert_eq!(ips[0].ip, "192.168.1.10");
        std::env::remove_var("KRONN_HOST_IPS");
    }
}
