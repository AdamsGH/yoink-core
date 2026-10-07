//! Confined provider transport: only source-proven hosts, HTTPS, no redirects,
//! no environment proxy, and public-only pinned DNS results. Resolver permits
//! are held by the blocking lookup itself, even if an HTTP waiter is cancelled.
use crate::DownloadError;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs},
    sync::{Arc, OnceLock},
    time::Duration,
};
struct ProviderDns;
fn denied() -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(std::io::Error::other(
        "Provider network resolution rejected",
    ))
}
fn allowed_host(host: &str) -> bool {
    matches!(
        host,
        "www.youtube.com" | "www.instagram.com" | "api.twitter.com"
    )
}
fn validated(found: Vec<SocketAddr>) -> Result<Addrs, Box<dyn std::error::Error + Send + Sync>> {
    if found.is_empty() || found.len() > 32 || found.iter().any(|a| !is_public(a.ip())) {
        return Err(denied());
    }
    Ok(Box::new(found.into_iter()))
}
impl Resolve for ProviderDns {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            if !allowed_host(&host) {
                return Err(denied());
            }
            static SLOTS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
            let permit = SLOTS
                .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(4)))
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| denied())?;
            let found = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                (host.as_str(), 443)
                    .to_socket_addrs()
                    .map(|a| a.take(33).collect::<Vec<_>>())
                    .unwrap_or_default()
            })
            .await
            .map_err(|_| denied())?;
            validated(found)
        })
    }
}
pub(super) fn client(timeout: Duration) -> Result<reqwest::Client, DownloadError> {
    reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(ProviderDns))
        .timeout(timeout)
        .build()
        .map_err(|_| DownloadError::InvalidConfig)
}
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => public4(v),
        IpAddr::V6(v) => public6(v),
    }
}
fn public4(v: Ipv4Addr) -> bool {
    let n = u32::from(v);
    // IANA special-purpose allocations that are not globally reachable.
    const BLOCKED: &[(u32, u8)] = &[
        (0, 8),
        (0x0a000000, 8),
        (0x64400000, 10),
        (0x7f000000, 8),
        (0xa9fe0000, 16),
        (0xac100000, 12),
        (0xc0000000, 24),
        (0xc0000200, 24),
        (0xc01fc400, 24),
        (0xc034c100, 24),
        (0xc0586300, 24),
        (0xc0a80000, 16),
        (0xc6120000, 15),
        (0xc6336400, 24),
        (0xcb007100, 24),
        (0xe0000000, 4),
        (0xf0000000, 4),
    ];
    !BLOCKED
        .iter()
        .any(|(base, p)| n & (u32::MAX << (32 - p)) == *base)
}
fn public6(v: Ipv6Addr) -> bool {
    let o = v.octets();
    if let Some(v4) = v.to_ipv4_mapped() {
        return public4(v4);
    }
    // Conservative global-unicast policy: only 2000::/3, excluding documentation,
    // transition/tunneling mechanisms, benchmarking, and the deprecated ORCHID block.
    let in_2000_3 = o[0] & 0xe0 == 0x20;
    let documentation = o[0..4] == [0x20, 0x01, 0x0d, 0xb8];
    let teredo = o[0..4] == [0x20, 0x01, 0x00, 0x00];
    let benchmarking = o[0..4] == [0x20, 0x01, 0x02, 0x00];
    let orchid = o[0..3] == [0x20, 0x01, 0x00] && (0x10..=0x2f).contains(&o[3]);
    let special_2001 = o[0] == 0x20 && o[1] == 0x01 && o[2] == 0 && o[3] <= 1;
    let six_to_four = o[0..2] == [0x20, 0x02];
    let documentation_3fff = o[0..2] == [0x3f, 0xff] && o[2] & 0xf0 == 0;
    let nat64_wkp = o[0..4] == [0x00, 0x64, 0xff, 0x9b];
    in_2000_3
        && !(documentation
            || teredo
            || benchmarking
            || orchid
            || special_2001
            || six_to_four
            || documentation_3fff
            || nat64_wkp)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_confinement_rejects_special_addresses_mixed_answers_and_hosts() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "192.168.1.1",
            "192.0.2.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!is_public(ip.parse().unwrap()));
        }
        assert!(is_public("8.8.8.8".parse().unwrap()));
        assert!(is_public("2606:4700:4700::1111".parse().unwrap()));
        assert!(
            validated(vec![
                "8.8.8.8:443".parse().unwrap(),
                "127.0.0.1:443".parse().unwrap()
            ])
            .is_err()
        );
        assert!(validated(vec![]).is_err());
        for host in [
            "localhost",
            "youtube.com.attacker.test",
            "www.youtube.com@evil",
            "127.0.0.1",
        ] {
            assert!(!allowed_host(host));
        }
    }
    #[tokio::test]
    async fn rejected_host_never_reaches_system_dns() {
        assert!(
            ProviderDns
                .resolve("localhost".parse().unwrap())
                .await
                .is_err()
        );
    }
}
