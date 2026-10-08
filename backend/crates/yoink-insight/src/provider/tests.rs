use super::*;
use std::{
    net::TcpListener,
    process::{Child, Command, Stdio},
};

const ROOT: &[u8] = include_bytes!("../../tests/fixtures/trust/root.der");
fn trusted() -> HttpCompletion {
    HttpCompletion::with_additional_roots(Duration::from_secs(3), 4096, &[ROOT]).unwrap()
}

#[test]
fn rejects_unbounded_and_malformed_trust() {
    let construct = |roots: &[&[u8]]| {
        HttpCompletion::with_additional_roots(Duration::from_secs(3), 4096, roots)
    };
    assert!(matches!(
        construct(&[]),
        Err(FixtureTrustError::RootCount(0))
    ));
    assert!(matches!(
        construct(&[ROOT; 9]),
        Err(FixtureTrustError::RootCount(9))
    ));
    assert!(matches!(
        construct(&[b""]),
        Err(FixtureTrustError::RootSize { index: 0, bytes: 0 })
    ));
    assert!(matches!(
        construct(&[&[0; 16385]]),
        Err(FixtureTrustError::RootSize {
            index: 0,
            bytes: 16385
        })
    ));
    for bad in [
        b"garbage".as_slice(),
        b"-----BEGIN CERTIFICATE-----\ninvalid\n-----END CERTIFICATE-----",
    ] {
        assert!(matches!(
            construct(&[ROOT, bad]),
            Err(FixtureTrustError::InvalidRoot { index: 1 })
        ));
    }
    assert!(matches!(
        HttpCompletion::with_additional_roots(Duration::ZERO, 4096, &[ROOT]),
        Err(FixtureTrustError::InvalidBudgets)
    ));
    assert!(matches!(
        HttpCompletion::with_additional_roots(Duration::from_secs(301), 4096, &[ROOT]),
        Err(FixtureTrustError::InvalidBudgets)
    ));
    assert!(matches!(
        HttpCompletion::with_additional_roots(Duration::from_secs(1), 1023, &[ROOT]),
        Err(FixtureTrustError::InvalidBudgets)
    ));
    assert!(matches!(
        HttpCompletion::with_additional_roots(Duration::from_secs(1), 4194305, &[ROOT]),
        Err(FixtureTrustError::InvalidBudgets)
    ));
    assert!(
        HttpCompletion::new(Duration::from_secs(3), 4096)
            .unwrap()
            .additional_roots
            .is_empty()
    );
}

#[test]
fn fixture_trust_does_not_relax_pinned_address_policy() {
    let completion = trusted();
    let public = "8.8.8.8:443".parse().unwrap();
    let private = "127.0.0.1:443".parse().unwrap();
    for addresses in [
        vec![],
        vec![private],
        vec![public, private],
        vec![public; 33],
        vec!["[::1]:443".parse().unwrap()],
    ] {
        assert!(matches!(
            completion.pinned_client("fixture.invalid", &addresses),
            Err(Error::Code("unsafe_endpoint"))
        ));
    }
    assert!(
        completion
            .pinned_client("fixture.invalid", &[public])
            .is_ok()
    );
}

struct TlsFixture {
    child: Child,
    port: u16,
}
impl Drop for TlsFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl TlsFixture {
    async fn start() -> Self {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let fixtures =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/trust");
        let child = Command::new("openssl")
            .args([
                "s_server",
                "-accept",
                &format!("127.0.0.1:{port}"),
                "-www",
                "-quiet",
                "-cert",
            ])
            .arg(fixtures.join("localhost.pem"))
            .arg("-key")
            .arg(fixtures.join("localhost.key"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("provider TLS fixture requires openssl on PATH");
        let mut fixture = Self { child, port };
        for _ in 0..100 {
            assert!(
                fixture.child.try_wait().unwrap().is_none(),
                "TLS fixture exited"
            );
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                return fixture;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("TLS fixture did not become ready");
    }
}

#[tokio::test]
async fn roots_survive_every_pinned_rebuild_and_hostname_checks_remain_enabled() {
    let fixture = TlsFixture::start().await;
    let url = format!("https://localhost:{}/", fixture.port);
    let completion = trusted();
    assert!(
        completion
            .client
            .get(&url)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let production = HttpCompletion::new(Duration::from_secs(3), 4096).unwrap();
    assert!(
        production.client.get(&url).send().await.is_err(),
        "production must not trust fixture root"
    );
    // Construct the exact BYOK client path with validated public answers, then
    // probe its TLS trust independently. No resolver or origin hook is exposed
    // and no private address is passed to the pinned builder.
    for _ in 0..2 {
        let pinned = completion
            .pinned_client("fixture.invalid", &["8.8.8.8:443".parse().unwrap()])
            .unwrap();
        assert!(pinned.get(&url).send().await.unwrap().status().is_success());
        assert!(
            pinned
                .get(format!("https://127.0.0.1:{}/", fixture.port))
                .send()
                .await
                .is_err(),
            "additional roots must not bypass hostname verification"
        );
    }
}

#[tokio::test]
async fn real_byok_rejects_private_dns_even_with_fixture_trust() {
    let completion = trusted();
    let route = Route {
        shape: Shape::OpenAi,
        base: "https://localhost/v1".into(),
        key: "fixture".into(),
        model: "test".into(),
        via: "byok",
    };
    assert!(matches!(
        completion.route_client(&route).await,
        Err(Error::Code("unsafe_endpoint"))
    ));
}
