//! Which addresses `fetch_url` is allowed to reach.
//!
//! The model chooses that URL. Everything else the harness sends a request to —
//! a search backend, an MCP server, a model provider — is a URL the user wrote
//! in a config file, and guarding those would only stop people pointing Taurus
//! at their own SearXNG box. `fetch_url` is the one place a network destination
//! is picked by something that read a web page five minutes ago, and it is the
//! only one guarded here.
//!
//! What that guards against is not exotic. `169.254.169.254` is the cloud
//! metadata endpoint on every major provider and answers with credentials to
//! anything that asks; `127.0.0.1` and `10.x` reach whatever the user happens to
//! be running unauthenticated on their own machine and network. A permission
//! prompt is not much of a defence either, because the URL shown is a plausible
//! one — the point of the attack is that the page asking for it looks ordinary.
//!
//! # What this does not close
//!
//! The name is resolved here and resolved again by the connection, so a name
//! that answers with a public address now and a private one a moment later gets
//! through. Closing that needs the connection pinned to the address that was
//! checked, which reqwest can only do per-client, not per-request. This raises
//! the bar from "no obstacle at all" to "requires controlling a DNS server and
//! winning a race", and the residual gap is documented rather than implied
//! away.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Whether an address belongs to this machine or the network around it.
pub fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_v4(v4),
        // An IPv4 address written as IPv6 is still that IPv4 address.
        // `::ffff:127.0.0.1` is the standard way past a guard that checks the
        // two families separately and forgets they overlap.
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_private_v4(v4),
            None => is_private_v6(v6),
        },
    }
}

fn is_private_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        // 169.254.0.0/16. Cloud metadata lives here; of everything on this
        // list, it is the one with credentials behind it.
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
        // 100.64.0.0/10, carrier-grade NAT.
        || (a == 100 && (64..128).contains(&b))
        // 240.0.0.0/4, reserved.
        || a >= 240
}

fn is_private_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        // fc00::/7, unique local.
        || (first & 0xfe00) == 0xfc00
        // fe80::/10, link-local unicast.
        || (first & 0xffc0) == 0xfe80
}

/// Refuses a URL whose host is, or resolves to, a private address.
///
/// A host given as a literal address is checked directly; anything else is
/// resolved, and *every* address it answers with has to be public. Taking the
/// whole set rather than the first matters: a name that returns one public
/// address and one loopback address is the cheapest version of this attack, and
/// which one gets connected to is not ours to decide.
pub async fn ensure_public(url: &reqwest::Url) -> Result<(), String> {
    let Some(host) = url.host_str() else {
        return Err("has no host".into());
    };

    if let Ok(ip) = host.parse::<IpAddr>() {
        return match is_private(ip) {
            true => Err(refusal(host, ip)),
            false => Ok(()),
        };
    }

    // Port is irrelevant to the answer but `to_socket_addrs` insists on one.
    let target = format!("{host}:{}", url.port_or_known_default().unwrap_or(80));
    let addresses = tokio::task::spawn_blocking(move || {
        use std::net::ToSocketAddrs;
        target.to_socket_addrs().map(|a| a.collect::<Vec<_>>())
    })
    .await
    .map_err(|_| "address lookup did not finish".to_string())?
    .map_err(|e| format!("could not resolve {host}: {e}"))?;

    if addresses.is_empty() {
        return Err(format!("{host} resolved to no addresses"));
    }
    for address in addresses {
        if is_private(address.ip()) {
            return Err(refusal(host, address.ip()));
        }
    }
    Ok(())
}

fn refusal(host: &str, ip: IpAddr) -> String {
    format!(
        "{host} is {ip}, an address on this machine or its private network. Taurus does not \
         fetch those, because a page can ask for one and the reply may be a credential. If this \
         is deliberate — a documentation server you run — set \"allow_private_hosts\": true in \
         search.json."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn the_cloud_metadata_address_is_private() {
        // The one that matters most: it answers unauthenticated, with
        // credentials, on every major cloud.
        assert!(is_private(ip("169.254.169.254")));
    }

    #[test]
    fn loopback_and_the_private_ranges_are_private() {
        for address in [
            "127.0.0.1",
            "127.9.9.9",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "0.0.0.0",
            "100.64.0.1",
            "255.255.255.255",
            "240.0.0.1",
        ] {
            assert!(is_private(ip(address)), "{address} should be refused");
        }
    }

    #[test]
    fn ordinary_public_addresses_are_allowed() {
        for address in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "172.32.0.1",
            "99.1.1.1",
        ] {
            assert!(!is_private(ip(address)), "{address} should be allowed");
        }
    }

    #[test]
    fn an_ipv4_address_wearing_ipv6_clothes_is_still_that_address() {
        // The classic bypass. `::ffff:127.0.0.1` is loopback, and a guard that
        // checks the families separately waves it through.
        assert!(is_private(ip("::ffff:127.0.0.1")));
        assert!(is_private(ip("::ffff:169.254.169.254")));
        assert!(!is_private(ip("::ffff:8.8.8.8")));
    }

    #[test]
    fn ipv6_local_ranges_are_private() {
        for address in ["::1", "::", "fc00::1", "fd12:3456::1", "fe80::1", "ff02::1"] {
            assert!(is_private(ip(address)), "{address} should be refused");
        }
    }

    #[test]
    fn public_ipv6_is_allowed() {
        assert!(!is_private(ip("2606:4700:4700::1111")));
    }

    #[tokio::test]
    async fn a_literal_private_url_is_refused_without_a_lookup() {
        let url = reqwest::Url::parse("http://127.0.0.1:8080/admin").unwrap();
        let error = ensure_public(&url).await.unwrap_err();
        assert!(error.contains("127.0.0.1"), "{error}");
        assert!(error.contains("allow_private_hosts"), "{error}");
    }

    #[tokio::test]
    async fn the_metadata_url_is_refused() {
        let url = reqwest::Url::parse("http://169.254.169.254/latest/meta-data/").unwrap();
        assert!(ensure_public(&url).await.is_err());
    }

    #[tokio::test]
    async fn a_literal_public_url_is_allowed() {
        let url = reqwest::Url::parse("https://93.184.216.34/").unwrap();
        assert!(ensure_public(&url).await.is_ok());
    }
}
