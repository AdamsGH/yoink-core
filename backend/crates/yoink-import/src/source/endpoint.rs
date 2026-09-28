use std::{net::IpAddr, str::FromStr};

use tokio_postgres::{Config, config::Host};

/// Check both DNS names and address overrides before permitting plaintext.
pub(super) fn is_local(config: &Config) -> bool {
    let hosts = config.get_hosts();
    if hosts.is_empty()
        || hosts.iter().any(|host| match host {
            Host::Tcp(host) => !IpAddr::from_str(host).is_ok_and(|ip| ip.is_loopback()),
            Host::Unix(path) => !path.is_absolute(),
        })
    {
        return false;
    }
    let addresses = config.get_hostaddrs();
    addresses.is_empty()
        || (addresses.len() == hosts.len() && addresses.iter().all(IpAddr::is_loopback))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permits_only_explicit_local_endpoints() {
        for dsn in [
            "host=127.0.0.1",
            "host=::1",
            "host=/run/postgresql",
            "host=127.0.0.1,::1 hostaddr=127.0.0.1,::1",
        ] {
            assert!(is_local(&dsn.parse().unwrap()), "{dsn}");
        }
        for dsn in [
            "",
            "host=localhost",
            "host=192.0.2.1",
            "host=0.0.0.0",
            "host=127.0.0.1 hostaddr=192.0.2.1",
            "host=127.0.0.1 hostaddr=0.0.0.0",
            "host=127.0.0.1,::1 hostaddr=127.0.0.1,192.0.2.1",
            "host=127.0.0.1,example.test",
            "host=127.0.0.1,::1 hostaddr=127.0.0.1",
        ] {
            assert!(!is_local(&dsn.parse().unwrap()), "{dsn}");
        }
    }
}
