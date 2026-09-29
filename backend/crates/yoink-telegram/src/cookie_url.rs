use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use teloxide::{prelude::*, types::Message};
use url::{Host, Url};
use yoink_domain::DeliveryTarget;
use yoink_downloads::{CookieDomain, DownloadService};
use yoink_i18n::{Key, Language};

const MAX_BYTES: usize = 512 * 1024;
const MAX_URL: usize = 4096;
const REDIRECTS: usize = 5;

#[derive(Clone)]
pub struct CookieUrlConfig {
    timeout: Duration,
    private_origins: Vec<String>,
    slots: Arc<tokio::sync::Semaphore>,
}
impl std::fmt::Debug for CookieUrlConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CookieUrlConfig")
            .field("timeout", &self.timeout)
            .field("private_origins", &"[REDACTED]")
            .finish()
    }
}
#[derive(Clone, Copy, Debug)]
pub struct CookieUrlConfigError;
impl std::fmt::Display for CookieUrlConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid cookie URL configuration")
    }
}
impl std::error::Error for CookieUrlConfigError {}
impl CookieUrlConfig {
    pub fn new(
        timeout: Duration,
        private_origins: Vec<String>,
    ) -> Result<Self, CookieUrlConfigError> {
        if !(1..=60).contains(&timeout.as_secs())
            || timeout.subsec_nanos() != 0
            || private_origins.len() > 16
        {
            return Err(CookieUrlConfigError);
        }
        let mut origins = Vec::new();
        for origin in private_origins {
            let u = Url::parse(&origin).map_err(|_| CookieUrlConfigError)?;
            let ip = match u.host() {
                Some(Host::Ipv4(v)) => IpAddr::V4(v),
                Some(Host::Ipv6(v)) => IpAddr::V6(v),
                _ => return Err(CookieUrlConfigError),
            };
            let scheme_end = origin.find("://").ok_or(CookieUrlConfigError)?;
            let authority = &origin[scheme_end + 3..]
                .split(['/', '?', '#'])
                .next()
                .ok_or(CookieUrlConfigError)?;
            let port_text = if authority.starts_with('[') {
                authority.rsplit_once("]:").map(|(_, p)| p)
            } else {
                authority.rsplit_once(':').map(|(_, p)| p)
            }
            .ok_or(CookieUrlConfigError)?;
            let port = port_text.parse::<u16>().map_err(|_| CookieUrlConfigError)?;
            if port == 0
                || authority.contains('@')
                || !matches!(u.scheme(), "http" | "https")
                || !u.username().is_empty()
                || u.password().is_some()
                || u.query().is_some()
                || u.fragment().is_some()
                || !matches!(u.path(), "/" | "")
            {
                return Err(CookieUrlConfigError);
            }
            let canonical = format!("{}://{}:{}", u.scheme(), host_text(ip), port);
            if origin != canonical && origin != format!("{canonical}/") {
                return Err(CookieUrlConfigError);
            }
            origins.push(canonical);
        }
        Ok(Self {
            timeout,
            private_origins: origins,
            slots: Arc::new(tokio::sync::Semaphore::new(4)),
        })
    }
}
impl Default for CookieUrlConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            private_origins: vec![],
            slots: Arc::new(tokio::sync::Semaphore::new(4)),
        }
    }
}
fn host_text(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v) => v.to_string(),
        IpAddr::V6(v) => format!("[{v}]"),
    }
}

#[derive(Clone, PartialEq)]
pub(super) enum Action {
    Import(String, Option<CookieDomain>),
    Invalid,
}
impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CookieUrl([REDACTED])")
    }
}
pub(super) fn parse(args: &[&str]) -> Option<Action> {
    if args.first().is_none_or(|s| {
        !s.get(..7)
            .is_some_and(|p| p.eq_ignore_ascii_case("http://"))
            && !s
                .get(..8)
                .is_some_and(|p| p.eq_ignore_ascii_case("https://"))
    }) {
        return None;
    }
    Some(if args.len() > 2 {
        Action::Invalid
    } else {
        let domain = args.get(1).and_then(|s| CookieDomain::parse(s).ok());
        if args.get(1).is_some() && domain.is_none() {
            Action::Invalid
        } else {
            Action::Import(args[0].to_string(), domain)
        }
    })
}

fn safe_url(input: &str) -> Option<Url> {
    if input.len() > MAX_URL
        || input.is_empty()
        || input.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let bytes = input.as_bytes();
    for (i, _) in bytes.iter().enumerate().filter(|(_, b)| **b == b'%') {
        let hi = (*bytes.get(i + 1)? as char).to_digit(16)?;
        let lo = (*bytes.get(i + 2)? as char).to_digit(16)?;
        let decoded = (hi * 16 + lo) as u8;
        if decoded <= 0x20 || decoded == 0x7f {
            return None;
        }
    }
    let u = Url::parse(input).ok()?;
    let authority = input.split_once("://")?.1.split(['/', '?', '#']).next()?;
    if authority.contains('@')
        || !matches!(u.scheme(), "http" | "https")
        || !u.username().is_empty()
        || u.password().is_some()
        || u.fragment().is_some()
        || u.host().is_none()
    {
        return None;
    }
    Some(u)
}
fn private_exception(url: &Url, cfg: &CookieUrlConfig) -> bool {
    let ip = match url.host() {
        Some(Host::Ipv4(v)) => IpAddr::V4(v),
        Some(Host::Ipv6(v)) => IpAddr::V6(v),
        _ => return false,
    };
    cfg.private_origins.iter().any(|o| {
        Url::parse(o).is_ok_and(|x| {
            x.scheme() == url.scheme()
                && x.host() == url.host()
                && x.port_or_known_default() == url.port_or_known_default()
        })
    }) && !is_public(ip)
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

fn redirect_target(current: &Url, location: &str) -> Option<Url> {
    if location.is_empty()
        || location
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    // Validate lexical bytes before URL parsing, which otherwise normalizes controls.
    for (i, _) in location
        .as_bytes()
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == b'%')
    {
        let hi = (*location.as_bytes().get(i + 1)? as char).to_digit(16)?;
        let lo = (*location.as_bytes().get(i + 2)? as char).to_digit(16)?;
        let decoded = (hi * 16 + lo) as u8;
        if decoded <= 0x20 || decoded == 0x7f {
            return None;
        }
    }
    if location.split_once("://").is_some_and(|(_, rest)| {
        rest.split(['/', '?', '#'])
            .next()
            .is_some_and(|authority| authority.contains('@'))
    }) {
        return None;
    }
    let resolved = current.join(location).ok()?;
    safe_url(resolved.as_str())
}
fn redirect_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}
fn validated_redirect(current: &Url, status: reqwest::StatusCode, location: &str) -> Option<Url> {
    if !redirect_status(status) {
        return None;
    }
    let next = redirect_target(current, location)?;
    if current.scheme() == "https" && next.scheme() == "http" {
        return None;
    }
    Some(next)
}
fn inferred_domain(host: &str) -> &str {
    let host = host.strip_prefix("www.").unwrap_or(host);
    match host {
        "youtu.be" | "m.youtube.com" | "music.youtube.com" => "youtube.com",
        "x.com" | "m.twitter.com" => "twitter.com",
        "m.instagram.com" => "instagram.com",
        "m.tiktok.com" => "tiktok.com",
        "m.facebook.com" => "facebook.com",
        "m.reddit.com" | "old.reddit.com" => "reddit.com",
        other => other,
    }
}
async fn resolve(url: &Url) -> Result<Vec<SocketAddr>, ()> {
    let port = url.port_or_known_default().ok_or(())?;
    let host = match url.host().ok_or(())? {
        Host::Ipv4(v) => return Ok(vec![SocketAddr::new(IpAddr::V4(v), port)]),
        Host::Ipv6(v) => return Ok(vec![SocketAddr::new(IpAddr::V6(v), port)]),
        Host::Domain(d) => d.to_owned(),
    };
    let found = bounded_lookup(DNS_SLOTS.clone(), move || {
        std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), port))
            .map(|it| it.collect::<Vec<_>>())
            .unwrap_or_default()
    })
    .await?;
    validate_resolution(found)
}
fn validate_resolution(found: Vec<SocketAddr>) -> Result<Vec<SocketAddr>, ()> {
    if found.is_empty() || found.iter().any(|s| !is_public(s.ip())) {
        return Err(());
    }
    Ok(found)
}
async fn bounded_lookup<T, F>(slots: Arc<tokio::sync::Semaphore>, lookup: F) -> Result<T, ()>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let permit = slots.acquire_owned().await.map_err(|_| ())?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("cookie-url-dns".into())
        .spawn(move || {
            let _permit = permit;
            let _ = tx.send(lookup());
        })
        .map_err(|_| ())?;
    rx.await.map_err(|_| ())
}
static DNS_SLOTS: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(4)));
async fn fetch(source: &str, cfg: &CookieUrlConfig) -> Result<String, ()> {
    let mut url = safe_url(source).ok_or(())?;
    for hop in 0..=REDIRECTS {
        let addrs = resolve(&url).await?;
        let exception = private_exception(&url, cfg);
        if !exception && addrs.iter().any(|s| !is_public(s.ip())) {
            return Err(());
        }
        let host = url.host_str().ok_or(())?.to_owned();
        let client = reqwest::Client::builder()
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(cfg.timeout)
            .timeout(cfg.timeout)
            .resolve_to_addrs(&host, &addrs)
            .build()
            .map_err(|_| ())?;
        let response = client
            .get(url.clone())
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send()
            .await
            .map_err(|_| ())?;
        if response.status().is_redirection() {
            if hop == REDIRECTS {
                return Err(());
            }
            let loc = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or(())?;
            url = validated_redirect(&url, response.status(), loc).ok_or(())?;
            continue;
        }
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return Err(());
        }
        if response
            .headers()
            .get(reqwest::header::CONTENT_ENCODING)
            .is_some_and(|value| !value.as_bytes().eq_ignore_ascii_case(b"identity"))
        {
            return Err(());
        }
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
            if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
                return Err(());
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err(());
        }
        return String::from_utf8(bytes).map_err(|_| ());
    }
    Err(())
}

pub(super) struct ImportDependencies<'a> {
    pub service: Option<&'a DownloadService>,
    pub config: Option<CookieUrlConfig>,
    pub actor: u64,
    pub target: DeliveryTarget,
    pub action: Action,
    pub language: Language,
}
pub(super) async fn handle(bot: &Bot, msg: &Message, deps: ImportDependencies<'_>) {
    let ImportDependencies {
        service,
        config,
        actor,
        target,
        action,
        language,
    } = deps;
    let guidance = || crate::translate(language, Key::CookieUrlUsage, &[]);
    let Action::Import(source, explicit) = action else {
        let _ = bot.send_message(msg.chat.id, guidance()).await;
        return;
    };
    let Some(service) = service else {
        let _ = bot
            .send_message(
                msg.chat.id,
                crate::translate(language, Key::DownloadsUnavailable, &[]),
            )
            .await;
        return;
    };
    let Some(config) = config else {
        let _ = bot.send_message(msg.chat.id, guidance()).await;
        return;
    };
    if !msg.chat.is_private()
        || msg.thread_id.is_some()
        || msg.sender_chat.is_some()
        || msg
            .from
            .as_ref()
            .is_none_or(|u| u.is_bot || u.id.0 != actor)
        || actor == 0
    {
        return;
    }
    let parsed = safe_url(&source);
    let Some(url) = parsed else {
        let _ = bot.send_message(msg.chat.id, guidance()).await;
        return;
    };
    let host = inferred_domain(url.host_str().unwrap_or(""));
    let domain = explicit.or_else(|| CookieDomain::parse(host).ok());
    let Some(domain) = domain else {
        let _ = bot.send_message(msg.chat.id, guidance()).await;
        return;
    };
    enum Outcome {
        Saved,
        Replay,
        Denied(yoink_downloads::DownloadError),
    }
    enum FetchOutcome {
        Body(String),
        Replay,
        Denied(yoink_downloads::DownloadError),
    }
    let fetched = tokio::time::timeout(config.timeout, async {
        let _permit = config.slots.clone().acquire_owned().await.map_err(|_| ())?;
        match service
            .personal_cookie_upload_allowed(actor, target, msg.id.0)
            .await
        {
            Ok(false) => return Ok(FetchOutcome::Replay),
            Err(e) => return Ok(FetchOutcome::Denied(e)),
            Ok(true) => {}
        }
        let body = fetch(&source, &config).await.map_err(|_| ())?;
        Ok(FetchOutcome::Body(body))
    })
    .await;
    let outcome = match fetched {
        Ok(Ok(FetchOutcome::Body(body))) => match service
            .store_personal_cookie(actor, target, msg.id.0, &domain, &body)
            .await
        {
            Ok(Some(_)) => Outcome::Saved,
            Ok(None) => Outcome::Replay,
            Err(e) => Outcome::Denied(e),
        },
        Ok(Ok(FetchOutcome::Replay)) => Outcome::Replay,
        Ok(Ok(FetchOutcome::Denied(e))) => Outcome::Denied(e),
        Ok(Err(())) | Err(_) => {
            let _ = bot
                .send_message(
                    msg.chat.id,
                    crate::translate(language, Key::CookieUrlFetchError, &[]),
                )
                .await;
            return;
        }
    };
    let text = match outcome {
        Outcome::Saved => crate::translate(language, Key::CookieUrlSaved, &[]),
        Outcome::Replay => crate::translate(language, Key::CookieProcessed, &[]),
        Outcome::Denied(e) => crate::download_error_response(e, language),
    };
    let _ = bot.send_message(msg.chat.id, text).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn urls_and_origins_are_strict_and_secrets_redacted() {
        for s in [
            "http://user@host/x",
            "http://host/x#f",
            "http://host/a b",
            "ftp://host/x",
            "http://host/\n",
            "http://host/a\u{2003}b",
            "http://host/a\u{0085}b",
            "http://host/%0d%0a",
            "http://host/%",
        ] {
            assert!(safe_url(s).is_none(), "{s:?}")
        }
        assert!(safe_url(&format!("http://host/{}", "x".repeat(MAX_URL))).is_none());
        assert!(format!("{:?}", CookieUrlConfig::default()).contains("REDACTED"));
        assert!(
            CookieUrlConfig::new(Duration::from_secs(2), vec!["http://127.0.0.1:80/".into()])
                .is_ok()
        );
        for (t, o) in [
            (Duration::ZERO, vec![]),
            (Duration::from_secs(61), vec![]),
            (
                Duration::from_secs(2),
                vec!["http://127.0.0.1:1".into(); 17],
            ),
        ] {
            assert!(CookieUrlConfig::new(t, o).is_err())
        }
        for s in [
            "http://localhost:9/",
            "http://127.0.0.1/",
            "http://127.0.0.1:0/",
            "http://127.0.0.1:9/a",
            "http://u@127.0.0.1:9/",
            "http://127.0.0.1:9/?q=x",
        ] {
            assert!(
                CookieUrlConfig::new(Duration::from_secs(2), vec![s.into()]).is_err(),
                "{s}"
            )
        }
    }
    #[test]
    fn private_origin_exception_is_exact() {
        let cfg = CookieUrlConfig::new(Duration::from_secs(2), vec!["http://127.0.0.1:80".into()])
            .unwrap();
        assert!(private_exception(
            &safe_url("http://127.0.0.1/a").unwrap(),
            &cfg
        ));
        for s in [
            "https://127.0.0.1/a",
            "http://127.0.0.1:81/a",
            "http://127.0.0.2/a",
        ] {
            assert!(!private_exception(&safe_url(s).unwrap(), &cfg), "{s}")
        }
    }
    #[test]
    fn mixed_public_and_private_dns_answers_are_rejected() {
        let answers = vec![
            "8.8.8.8:443".parse().unwrap(),
            "127.0.0.1:443".parse().unwrap(),
        ];
        assert!(validate_resolution(answers).is_err());
        assert!(validate_resolution(Vec::new()).is_err());
        assert_eq!(
            validate_resolution(vec!["8.8.8.8:443".parse().unwrap()])
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn parser_keeps_legacy_list_commands_separate_and_redacts_source() {
        assert!(parse(&["https://example.test/cookies?token=secret"]).is_some());
        assert!(parse(&["https://example.test/cookies", "example.com"]).is_some());
        assert!(matches!(
            parse(&["https://example.test/cookies", "bad domain"]),
            Some(Action::Invalid)
        ));
        assert!(parse(&["clear"]).is_none());
        let a = parse(&["https://example.test/cookies?token=secret"]).unwrap();
        assert!(!format!("{a:?}").contains("secret"));
        assert!(matches!(
            parse(&["http://example.test", "a", "b"]),
            Some(Action::Invalid)
        ));
    }
    #[test]
    fn public_ip_classifier_rejects_special_and_mapped() {
        for s in [
            "0.0.0.0",
            "10.0.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "192.168.1.1",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "2001::1",
            "2002:0808::1",
            "2001:0000::1",
            "64:ff9b::808:808",
            "3fff::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:192.168.1.1",
        ] {
            assert!(!is_public(s.parse().unwrap()), "{s}")
        }
        for s in [
            "8.8.8.8",
            "1.1.1.1",
            "192.1.1.1",
            "192.0.1.1",
            "192.169.1.1",
            "198.17.1.1",
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
        ] {
            assert!(is_public(s.parse().unwrap()), "{s}")
        }
        for s in [
            "192.0.0.1",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.1.1",
            "198.51.100.1",
            "203.0.113.1",
        ] {
            assert!(!is_public(s.parse().unwrap()), "{s}")
        }
    }
    #[test]
    fn redirect_validator_checks_raw_location_before_resolution() {
        let current = safe_url("https://example.test/a/b").unwrap();
        assert_eq!(
            redirect_target(&current, "../next").unwrap().as_str(),
            "https://example.test/next"
        );
        for location in [" /next", "/bad\npath", "/%0a", "https://user@example.test/"] {
            assert!(
                redirect_target(&current, location).is_none(),
                "{location:?}"
            );
        }
        assert!(
            validated_redirect(
                &current,
                reqwest::StatusCode::FOUND,
                "http://example.test/next"
            )
            .is_none()
        );
        assert!(
            validated_redirect(&current, reqwest::StatusCode::MULTIPLE_CHOICES, "/next").is_none()
        );
        assert!(redirect_status(reqwest::StatusCode::MOVED_PERMANENTLY));
        assert!(redirect_status(reqwest::StatusCode::TEMPORARY_REDIRECT));
        assert!(redirect_status(reqwest::StatusCode::PERMANENT_REDIRECT));
        assert!(!redirect_status(reqwest::StatusCode::MULTIPLE_CHOICES));
    }
    #[test]
    fn legacy_domain_inference_strips_www_once() {
        assert_eq!(inferred_domain("www.m.youtube.com"), "youtube.com");
        assert_eq!(inferred_domain("www.www.example.test"), "www.example.test");
        assert_eq!(inferred_domain("old.reddit.com"), "reddit.com");
    }
    #[tokio::test]
    async fn local_http_fetch_limits_encoding_redirects_and_recovers() {
        async fn server(
            response: Vec<u8>,
            expected: Option<&'static str>,
        ) -> (String, tokio::task::JoinHandle<()>) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let handle = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut request = vec![0; 8192];
                let n = stream.read(&mut request).await.unwrap();
                if let Some(header) = expected {
                    assert!(
                        String::from_utf8_lossy(&request[..n])
                            .to_ascii_lowercase()
                            .contains(&header.to_ascii_lowercase())
                    );
                }
                stream.write_all(&response).await.unwrap();
                let _ = stream.shutdown().await;
            });
            (format!("http://{addr}/start"), handle)
        }
        async fn redirect_server() -> (String, tokio::task::JoinHandle<()>) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let handle = tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                for (index, response) in [
                    b"HTTP/1.1 302 Found\r\nLocation: /done\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".as_slice(),
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".as_slice(),
                ].into_iter().enumerate() {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = vec![0; 4096];
                    let n = stream.read(&mut request).await.unwrap();
                    let request = String::from_utf8_lossy(&request[..n]).to_ascii_lowercase();
                    assert!(!request.contains("authorization:"));
                    if index == 1 { assert!(request.starts_with("get /done ")); }
                    stream.write_all(response).await.unwrap();
                    let _ = stream.shutdown().await;
                }
            });
            (format!("http://{addr}/start"), handle)
        }
        fn config(url: &str) -> CookieUrlConfig {
            let u = Url::parse(url).unwrap();
            CookieUrlConfig::new(
                Duration::from_secs(2),
                vec![format!("http://127.0.0.1:{}", u.port().unwrap())],
            )
            .unwrap()
        }
        let body = "x".repeat(MAX_BYTES);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let (url, task) = server(response.into_bytes(), Some("accept-encoding: identity")).await;
        assert_eq!(fetch(&url, &config(&url)).await.unwrap().len(), MAX_BYTES);
        task.await.unwrap();

        for response in [
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", MAX_BYTES + 1, "x".repeat(MAX_BYTES + 1)),
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n80001\r\n".to_owned() + &"x".repeat(MAX_BYTES + 1) + "\r\n0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx".into(),
            "HTTP/1.1 500 Failure\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx".into(),
        ] {
            let (url, task) = server(response.into_bytes(), None).await;
            assert!(fetch(&url, &config(&url)).await.is_err());
            task.await.unwrap();
        }
        let (url, task) = server(
            b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\n\xff".to_vec(),
            None,
        )
        .await;
        assert!(fetch(&url, &config(&url)).await.is_err());
        task.await.unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_millis(1200)).await;
            use tokio::io::AsyncWriteExt;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await;
        });
        let slow = format!("http://{addr}/slow");
        let slow_config = CookieUrlConfig::new(
            Duration::from_secs(1),
            vec![format!("http://127.0.0.1:{}", addr.port())],
        )
        .unwrap();
        assert!(fetch(&slow, &slow_config).await.is_err());
        task.await.unwrap();

        let (url, task) = redirect_server().await;
        assert_eq!(fetch(&url, &config(&url)).await.unwrap(), "ok");
        task.await.unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            for _ in 0..=REDIRECTS {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 2048];
                let _ = stream.read(&mut request).await.unwrap();
                stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /again\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                let _ = stream.shutdown().await;
            }
        });
        let url = format!("http://{addr}/start");
        assert!(fetch(&url, &config(&url)).await.is_err());
        task.await.unwrap();
    }
    #[tokio::test]
    async fn cancelled_dns_waiter_does_not_release_blocked_resolver_permit() {
        let slots = Arc::new(tokio::sync::Semaphore::new(1));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let waiting = tokio::spawn(bounded_lookup(slots.clone(), move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        }));
        tokio::task::spawn_blocking(move || started_rx.recv().unwrap())
            .await
            .unwrap();
        waiting.abort();
        assert!(slots.clone().try_acquire_owned().is_err());
        release_tx.send(()).unwrap();
        let _permit = tokio::time::timeout(Duration::from_secs(1), slots.acquire())
            .await
            .unwrap()
            .unwrap();
    }
}
