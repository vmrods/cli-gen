use url::{Host, Url};

/// `localhost`, `127.0.0.0/8` or `::1`, where plain HTTP never leaves the machine.
#[must_use]
pub fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

/// HTTPS anywhere, or HTTP to the local machine.
#[must_use]
pub fn is_secure_transport(url: &Url) -> bool {
    url.scheme() == "https" || (url.scheme() == "http" && is_loopback(url))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap_or_else(|error| panic!("{raw}: {error}"))
    }

    #[test]
    fn is_secure_transport_allows_https_and_loopback_http_only() {
        for raw in [
            "https://api.example.com",
            "http://localhost:8080",
            "http://127.0.0.1",
            "http://[::1]:9",
        ] {
            assert!(is_secure_transport(&url(raw)), "{raw}");
        }
        for raw in [
            "http://api.example.com",
            "http://10.0.0.1",
            "ftp://localhost",
        ] {
            assert!(!is_secure_transport(&url(raw)), "{raw}");
        }
    }
}
