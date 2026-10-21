//! The outbound-URL guard: what this server is allowed to make an HTTP request
//! to, for destinations a tenant supplies.
//!
//! The repo's own security notes call webhook SSRF "the headline", and they are
//! right: `create_webhook` validated its URL as `!url.trim().is_empty()`, the
//! dispatcher then POSTed job payloads to it with plain `reqwest`, and the
//! cluster's egress is deliberately unrestricted
//! (`infra/k8s/charts/er-dev-namespace/templates/networkpolicy.yaml`). An admin
//! in any tenant could therefore aim the control plane at
//! `http://169.254.169.254/latest/meta-data/...`, at `localhost`, or at any
//! `*.svc.cluster.local` service, and the server would oblige.
//!
//! Two properties make this guard worth having rather than decorative:
//!
//! **It resolves the host and checks the ADDRESSES, not the name.** A name-only
//! denylist is bypassed by pointing your own domain at `127.0.0.1`, and a
//! check-at-registration-only guard is bypassed by DNS rebinding — resolve
//! public when the URL is registered, private when it is delivered. So the same
//! function runs at registration *and* before every delivery.
//!
//! **A host that does not resolve is not a rejection.** A tenant registering a
//! webhook before standing the endpoint up is doing something legitimate, and a
//! lookup failure is not evidence of a private address. The delivery-time check
//! is what makes that safe: if the name ever resolves somewhere it should not,
//! the delivery is refused then.
//!
//! The honest limit, which this module cannot fix: the network leg. Whether a
//! pod can reach IMDS at all is a NetworkPolicy question, verified by the manual
//! probe in `docs/infrastructure.md` §17. This is the application layer, and the
//! unit tests below prove only the application layer.

use std::net::IpAddr;

/// Whether a destination is subject to the guard.
///
/// There is no environment variable and no config switch for this, deliberately:
/// a guard that can be turned off by configuration is a guard that will be found
/// off in production. The only unguarded path is
/// [`crate::webhook::drain_once_with_egress`], which exists so the delivery
/// integration test can POST to a local listener, and which
/// `tests/egress_guard.rs` forbids product code from naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Egress {
    /// Enforce the guard. Every production caller.
    Guarded,
    /// Skip the guard. Tests only — see the type's note.
    Unguarded,
}

/// Why a destination was refused. Each variant renders as the reason a tenant
/// sees, so it says what is wrong without describing the internal topology.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UrlRejection {
    #[error("url is not a valid absolute URL")]
    Unparseable,
    #[error("url scheme must be https, not {scheme:?}")]
    Scheme { scheme: String },
    #[error("url must not embed credentials")]
    Credentials,
    #[error("url has no host")]
    NoHost,
    #[error("host {host:?} resolves to {addr}, which is {reason}")]
    BlockedAddress {
        host: String,
        addr: IpAddr,
        reason: &'static str,
    },
}

/// Why `addr` may not be a destination, or `None` if it is fine.
///
/// IPv4-in-IPv6 forms are unwrapped and then checked as IPv4: without that,
/// `::ffff:127.0.0.1` is a loopback address that no IPv6 range check catches,
/// which is the standard way past this kind of filter.
///
/// The order below is load-bearing. `Ipv6Addr::to_ipv4` also maps the deprecated
/// IPv4-*compatible* form `::a.b.c.d`, and `::1` is in that form — so unwrapping
/// first turns the IPv6 loopback into `0.0.0.1`, which is not loopback, not
/// private and not unspecified, and the guard waves THE loopback address
/// through. So: judge `::1` and `::` as IPv6 first, unwrap second.
#[must_use]
pub fn blocked_reason(addr: IpAddr) -> Option<&'static str> {
    if let IpAddr::V6(v6) = addr {
        if v6.is_loopback() {
            return Some("a loopback address");
        }
        if v6.is_unspecified() {
            return Some("the unspecified address");
        }
    }
    let addr = match addr {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped().or_else(|| v6.to_ipv4()) {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 @ IpAddr::V4(_) => v4,
    };
    match addr {
        IpAddr::V4(v4) => {
            if v4.is_loopback() {
                Some("a loopback address")
            } else if v4.is_link_local() {
                // 169.254.0.0/16 — cloud instance metadata lives here.
                Some("a link-local address")
            } else if v4.is_private() {
                Some("a private address")
            } else if v4.is_unspecified() {
                Some("the unspecified address")
            } else if v4.is_broadcast() || v4.is_multicast() {
                Some("a broadcast or multicast address")
            } else if v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]) {
                // 100.64.0.0/10, carrier-grade NAT — the EKS pod CIDR in some
                // VPC-CNI layouts, so pod-to-pod is reachable through it.
                Some("a carrier-grade NAT address")
            } else {
                None
            }
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                Some("a loopback address")
            } else if v6.is_unspecified() {
                Some("the unspecified address")
            } else if v6.is_multicast() {
                Some("a multicast address")
            } else if (v6.segments()[0] & 0xffc0) == 0xfe80 {
                Some("a link-local address")
            } else if (v6.segments()[0] & 0xfe00) == 0xfc00 {
                Some("a unique-local address")
            } else {
                None
            }
        }
    }
}

/// Everything about a URL that can be judged without a DNS lookup: it parses,
/// the scheme is https, it carries no credentials, and it names a host. Split
/// out so registration can reject the obvious cases synchronously and so the
/// unit tests do not need a resolver.
///
/// # Errors
/// Returns the first applicable [`UrlRejection`].
pub fn check_static(url: &str) -> Result<reqwest::Url, UrlRejection> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|_| UrlRejection::Unparseable)?;
    if parsed.scheme() != "https" {
        return Err(UrlRejection::Scheme {
            scheme: parsed.scheme().to_string(),
        });
    }
    // `https://user:pass@host/` would send the credentials to whatever the host
    // turns out to be, and some proxies log them.
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(UrlRejection::Credentials);
    }
    match parsed.host_str() {
        None | Some("") => Err(UrlRejection::NoHost),
        Some(_) => Ok(parsed),
    }
}

/// The full check: [`check_static`], then resolve the host and reject if ANY
/// resolved address is one we must not reach.
///
/// Every address is checked, not just the first: a host that resolves to one
/// public and one private address would otherwise be reachable about half the
/// time, depending on resolver ordering.
///
/// A lookup that fails is NOT a rejection — see the module note.
///
/// # Errors
/// Returns the first applicable [`UrlRejection`].
pub async fn check(url: &str, egress: Egress) -> Result<(), UrlRejection> {
    let parsed = match egress {
        Egress::Unguarded => return Ok(()),
        Egress::Guarded => check_static(url)?,
    };
    let raw = parsed.host_str().unwrap_or_default();
    // An IPv6 literal's `host_str` keeps its brackets (`[::1]`), which is not a
    // resolvable name — so routing literals through DNS made `lookup_host`
    // fail, and "a lookup failure is not a rejection" then WAVED THROUGH the
    // IPv6 loopback. Literals are judged directly, before any resolution.
    let host = raw
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    if let Ok(addr) = host.parse::<IpAddr>() {
        return match blocked_reason(addr) {
            Some(reason) => Err(UrlRejection::BlockedAddress { host, addr, reason }),
            None => Ok(()),
        };
    }
    let port = parsed.port_or_known_default().unwrap_or(443);
    let Ok(addrs) = tokio::net::lookup_host((host.as_str(), port)).await else {
        return Ok(());
    };
    let addrs: Vec<IpAddr> = addrs.map(|s| s.ip()).collect();
    for addr in addrs {
        if let Some(reason) = blocked_reason(addr) {
            return Err(UrlRejection::BlockedAddress { host, addr, reason });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scheme_must_be_https() {
        for url in [
            "http://example.com/hook",
            "file:///etc/passwd",
            "gopher://example.com/",
            "ftp://example.com/",
        ] {
            assert!(
                matches!(check_static(url), Err(UrlRejection::Scheme { .. })),
                "{url} should be refused for its scheme"
            );
        }
        assert!(check_static("https://example.com/hook").is_ok());
    }

    #[test]
    fn credentials_and_missing_hosts_are_refused() {
        assert_eq!(
            check_static("https://user:pass@example.com/"),
            Err(UrlRejection::Credentials)
        );
        assert_eq!(
            check_static("https://user@example.com/"),
            Err(UrlRejection::Credentials)
        );
        assert_eq!(check_static("not a url"), Err(UrlRejection::Unparseable));
        assert_eq!(check_static(""), Err(UrlRejection::Unparseable));
    }

    /// The address ranges that matter, named one by one so a future edit that
    /// drops one fails here rather than in a cluster.
    #[test]
    fn the_addresses_a_tenant_must_not_reach() {
        let blocked = [
            ("127.0.0.1", "a loopback address"),
            ("127.1.2.3", "a loopback address"),
            // The one this guard exists for: cloud instance metadata.
            ("169.254.169.254", "a link-local address"),
            ("10.0.0.5", "a private address"),
            ("172.16.0.1", "a private address"),
            ("172.31.255.255", "a private address"),
            ("192.168.1.1", "a private address"),
            ("0.0.0.0", "the unspecified address"),
            ("100.64.0.1", "a carrier-grade NAT address"),
            ("::1", "a loopback address"),
            ("fe80::1", "a link-local address"),
            ("fc00::1", "a unique-local address"),
            ("fd12:3456::1", "a unique-local address"),
            ("::", "the unspecified address"),
            // The classic filter bypass: a loopback address wearing IPv6.
            ("::ffff:127.0.0.1", "a loopback address"),
            ("::ffff:169.254.169.254", "a link-local address"),
            // The deprecated IPv4-compatible form, which is what made `::1`
            // slip through an earlier version of this function: unwrapping
            // before judging turned the IPv6 loopback into `0.0.0.1`.
            ("::127.0.0.1", "a loopback address"),
            ("::169.254.169.254", "a link-local address"),
        ];
        for (ip, reason) in blocked {
            assert_eq!(
                blocked_reason(ip.parse().unwrap()),
                Some(reason),
                "{ip} must be blocked"
            );
        }

        // 172.32 is outside 172.16/12, and these are ordinary public addresses.
        for ip in ["1.1.1.1", "93.184.216.34", "172.32.0.1", "2606:4700::1111"] {
            assert_eq!(
                blocked_reason(ip.parse().unwrap()),
                None,
                "{ip} is public and must be allowed"
            );
        }
    }

    #[tokio::test]
    async fn an_ip_literal_is_checked_without_dns() {
        // No DNS involved, so this is hermetic.
        let err = check("https://169.254.169.254/latest/meta-data/", Egress::Guarded)
            .await
            .expect_err("IMDS must be refused");
        assert!(
            matches!(err, UrlRejection::BlockedAddress { reason, .. } if reason == "a link-local address"),
            "{err}"
        );
        assert!(
            matches!(
                check("https://127.0.0.1:8080/hook", Egress::Guarded).await,
                Err(UrlRejection::BlockedAddress { .. })
            ),
            "loopback with an explicit port must be refused"
        );
        // Bracketed IPv6 literals. These are the regression: `host_str` keeps
        // the brackets, so sending them through the resolver failed to parse,
        // and "a lookup failure is not a rejection" let the IPv6 loopback
        // straight through.
        for url in [
            "https://[::1]/hook",
            "https://[::1]:8443/hook",
            "https://[::ffff:127.0.0.1]/hook",
            "https://[fe80::1]/hook",
            "https://[fc00::1]/hook",
        ] {
            assert!(
                matches!(
                    check(url, Egress::Guarded).await,
                    Err(UrlRejection::BlockedAddress { .. })
                ),
                "{url} must be refused"
            );
        }
        // A public IPv6 literal still passes, so the bracket handling did not
        // turn into "reject every IPv6 address".
        assert!(
            check("https://[2606:4700::1111]/hook", Egress::Guarded)
                .await
                .is_ok()
        );
        // And the test seam really does skip the check, or the delivery
        // integration test would be proving nothing.
        assert!(
            check("http://127.0.0.1:8080/hook", Egress::Unguarded)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_host_that_does_not_resolve_is_allowed() {
        // `.invalid` is reserved never to resolve (RFC 2606), so a tenant
        // registering an endpoint they have not stood up yet is not refused.
        assert!(
            check("https://nonexistent.invalid/hook", Egress::Guarded)
                .await
                .is_ok()
        );
    }
}
