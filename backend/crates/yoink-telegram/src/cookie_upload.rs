use teloxide::{prelude::*, types::Message};
use url::Url;
use yoink_domain::DeliveryTarget;
use yoink_downloads::{CookieDomain, DownloadService};
use yoink_i18n::Key;

use crate::{download_error_response, translate};
use std::{io::Read, path::PathBuf, sync::Arc, time::Duration};

const MAX_BYTES: usize = 512 * 1024;

#[derive(Clone)]
pub struct CookieUploadConfig {
    timeout: Duration,
    local_files: Option<LocalFiles>,
}

#[derive(Clone)]
struct LocalFiles {
    remote_root: String,
    local_root: PathBuf,
    read_slots: Arc<tokio::sync::Semaphore>,
}

impl std::fmt::Debug for CookieUploadConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CookieUploadConfig")
            .field("timeout", &self.timeout)
            .field(
                "local_files",
                &self.local_files.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

impl CookieUploadConfig {
    pub fn new(timeout: Duration) -> Result<Self, CookieUploadConfigError> {
        if !(1..=60).contains(&timeout.as_secs()) || timeout.subsec_nanos() != 0 {
            return Err(CookieUploadConfigError);
        }
        Ok(Self {
            timeout,
            local_files: None,
        })
    }

    pub fn with_local_files(
        mut self,
        remote_root: PathBuf,
        local_root: PathBuf,
    ) -> Result<Self, CookieUploadConfigError> {
        let remote = remote_root.to_str().ok_or(CookieUploadConfigError)?;
        let local = local_root.to_str().ok_or(CookieUploadConfigError)?;
        if !validated_absolute(remote) || !validated_absolute(local) {
            return Err(CookieUploadConfigError);
        }
        self.local_files = Some(LocalFiles {
            remote_root: remote.to_owned(),
            local_root,
            read_slots: Arc::new(tokio::sync::Semaphore::new(1)),
        });
        Ok(self)
    }
}

impl Default for CookieUploadConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            local_files: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CookieUploadConfigError;

impl std::fmt::Display for CookieUploadConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid cookie upload configuration")
    }
}
impl std::error::Error for CookieUploadConfigError {}

pub(super) fn file_base(api_url: &Url, token: &str) -> Url {
    let mut base = api_url.clone();
    base.set_path(&format!("/file/bot{token}/"));
    base.set_query(None);
    base.set_fragment(None);
    base
}

fn validated_absolute(path: &str) -> bool {
    path.len() <= 1024
        && path.starts_with('/')
        && path != "/"
        && path.is_ascii()
        && path[1..].split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part.len() <= 255
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-:~".contains(&b))
        })
}

fn validated_component(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && part.len() <= 255
        && part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn local_relative_path(remote_root: &str, absolute: &str) -> Option<String> {
    if !validated_absolute(absolute) || !absolute.starts_with(remote_root) {
        return None;
    }
    let suffix = absolute.strip_prefix(remote_root)?.strip_prefix('/')?;
    validated_path(suffix).then(|| suffix.to_owned())
}

async fn bounded_local_read(
    slots: Arc<tokio::sync::Semaphore>,
    read: impl FnOnce() -> Result<String, FetchError> + Send + 'static,
) -> Result<String, FetchError> {
    // A timeout cannot cancel kernel filesystem I/O. Keep the permit in the
    // blocking closure until it actually exits, including after caller cancellation.
    let permit = slots
        .acquire_owned()
        .await
        .map_err(|_| FetchError::Transport)?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    // Do not leave an uninterruptible filesystem read in Tokio's blocking pool:
    // runtime shutdown waits for that pool. One detached, permit-owned thread
    // bounds outstanding I/O without holding up the async runtime's shutdown.
    std::thread::Builder::new()
        .name("cookie-file-read".to_owned())
        .spawn(move || {
            let _permit = permit;
            let _ = sender.send(read());
        })
        .map_err(|_| FetchError::Transport)?;
    receiver.await.map_err(|_| FetchError::Transport)?
}

fn read_local_file(root: PathBuf, relative: String) -> Result<String, FetchError> {
    use rustix::fs::{FileType, Mode, OFlags, fstat, openat};
    use std::os::fd::OwnedFd;

    let root = root.to_str().ok_or(FetchError::Invalid)?;
    if !validated_absolute(root) || !validated_path(&relative) {
        return Err(FetchError::Invalid);
    }
    let mut directory: OwnedFd = openat(
        rustix::fs::CWD,
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| FetchError::Invalid)?;
    for component in root[1..].split('/') {
        directory = openat(
            &directory,
            component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| FetchError::Invalid)?;
    }
    let mut parts = relative.split('/').peekable();
    while let Some(component) = parts.next() {
        if parts.peek().is_some() {
            directory = openat(
                &directory,
                component,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| FetchError::Invalid)?;
        } else {
            let file = openat(
                &directory,
                component,
                OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| FetchError::Invalid)?;
            let metadata = fstat(&file).map_err(|_| FetchError::Invalid)?;
            if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile
                || metadata.st_size < 0
                || metadata.st_size > MAX_BYTES as i64
            {
                return Err(FetchError::Invalid);
            }
            let mut reader = std::fs::File::from(file).take((MAX_BYTES + 1) as u64);
            let mut bytes = Vec::new();
            reader
                .read_to_end(&mut bytes)
                .map_err(|_| FetchError::Transport)?;
            return cookie_text(bytes);
        }
    }
    Err(FetchError::Invalid)
}

fn validated_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && path.is_ascii()
        && !path.starts_with('/')
        && !path
            .chars()
            .any(|c| matches!(c, '\\' | ':' | '?' | '#' | '%'))
        && path.split('/').all(|part| {
            part.len() <= 255
                && !part.is_empty()
                && part != "."
                && part != ".."
                && validated_component(part)
        })
}

fn cookie_text(bytes: Vec<u8>) -> Result<String, FetchError> {
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Err(FetchError::Invalid);
    }
    String::from_utf8(bytes).map_err(|_| FetchError::Invalid)
}

fn file_url(base: &Url, path: &str) -> Option<Url> {
    if !validated_path(path) {
        return None;
    }
    let mut url = base.clone();
    let prefix = url.path().to_owned();
    url.set_path(&format!("{prefix}{path}"));
    (url.origin() == base.origin() && url.query().is_none() && url.fragment().is_none())
        .then_some(url)
}

fn requested_upload(
    caption: Option<&str>,
    filename: Option<&str>,
    username: &str,
) -> Option<Result<CookieDomain, ()>> {
    let mut words = caption?.split_whitespace();
    let cmd = words.next()?;
    let command = cmd.strip_prefix('/')?;
    let (name, suffix) = command.split_once('@').unwrap_or((command, username));
    if !name.eq_ignore_ascii_case("cookie") {
        return None;
    }
    if !suffix.eq_ignore_ascii_case(username) {
        return None;
    }
    let args = words.collect::<Vec<_>>();
    match args.as_slice() {
        [] => filename_domain(filename).map(Ok).or(Some(Err(()))),
        [domain]
            if !["del", "clear", "token", "upload"]
                .iter()
                .any(|word| domain.eq_ignore_ascii_case(word)) =>
        {
            Some(CookieDomain::parse(domain).map_err(|_| ()))
        }
        _ => Some(Err(())),
    }
}

fn filename_domain(filename: Option<&str>) -> Option<CookieDomain> {
    let name = filename?;
    let (stem, _) = name.rsplit_once('.')?;
    if !stem.contains('.') {
        return None;
    }
    CookieDomain::parse(stem).ok()
}

pub(super) struct UploadDependencies<'a> {
    pub downloads: Option<&'a DownloadService>,
    pub config: Option<CookieUploadConfig>,
    pub base: &'a Url,
    pub client: &'a reqwest::Client,
}

pub(super) async fn handle_document(
    bot: &Bot,
    message: &Message,
    username: &str,
    actor: u64,
    target: DeliveryTarget,
    dependencies: UploadDependencies<'_>,
) {
    let Some(document) = message.document() else {
        return;
    };
    let language = crate::reply_language(
        dependencies.downloads,
        actor,
        message
            .from
            .as_ref()
            .and_then(|user| user.language_code.as_deref()),
    )
    .await;
    let Some(domain) = requested_upload(message.caption(), document.file_name.as_deref(), username)
    else {
        return;
    };
    let guidance = || translate(language, Key::CookieUploadGuidance, &[]);
    let domain = match domain {
        Ok(domain) => domain,
        Err(()) => {
            let _ = bot.send_message(message.chat.id, guidance()).await;
            return;
        }
    };
    let (Some(service), Some(config)) = (dependencies.downloads, dependencies.config) else {
        let _ = bot.send_message(message.chat.id, guidance()).await;
        return;
    };
    if document.file.size as usize > MAX_BYTES {
        let _ = bot.send_message(message.chat.id, guidance()).await;
        return;
    }
    let allowed = service
        .personal_cookie_upload_allowed(actor, target, message.id.0)
        .await;
    match allowed {
        Ok(false) => {
            let _ = bot
                .send_message(
                    message.chat.id,
                    translate(language, Key::CookieUploadReplay, &[]),
                )
                .await;
            return;
        }
        Err(error) => {
            let _ = bot
                .send_message(
                    message.chat.id,
                    crate::download_error_response(error, language),
                )
                .await;
            return;
        }
        Ok(true) => {}
    }
    let result = tokio::time::timeout(config.timeout, async {
        let file = bot
            .get_file(document.file.id.clone())
            .await
            .map_err(|_| FetchError::Transport)?;
        if file.meta.size as usize > MAX_BYTES {
            return Err(FetchError::Invalid);
        }
        let path = file.path;
        if path.starts_with('/') {
            let local = config.local_files.as_ref().ok_or(FetchError::Invalid)?;
            let relative =
                local_relative_path(&local.remote_root, &path).ok_or(FetchError::Invalid)?;
            let root = local.local_root.clone();
            return bounded_local_read(local.read_slots.clone(), move || {
                read_local_file(root, relative)
            })
            .await;
        }
        let url = file_url(dependencies.base, &path).ok_or(FetchError::Invalid)?;
        let mut response = dependencies
            .client
            .get(url)
            .send()
            .await
            .map_err(|_| FetchError::Transport)?;
        if response.status().is_server_error() {
            return Err(FetchError::Transport);
        }
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return Err(FetchError::Invalid);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| FetchError::Transport)? {
            if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
                return Err(FetchError::Invalid);
            }
            bytes.extend_from_slice(&chunk);
        }
        cookie_text(bytes)
    })
    .await;
    let content = match result {
        Ok(Ok(content)) => content,
        Ok(Err(FetchError::Invalid)) => {
            let _ = bot.send_message(message.chat.id, guidance()).await;
            return;
        }
        Ok(Err(FetchError::Transport)) | Err(_) => {
            let _ = bot
                .send_message(
                    message.chat.id,
                    translate(language, Key::CookieUploadRetry, &[]),
                )
                .await;
            return;
        }
    };
    match service
        .store_personal_cookie(actor, target, message.id.0, &domain, &content)
        .await
    {
        Ok(Some(count)) => {
            let text = translate(
                language,
                Key::CookieStored,
                &[("domain", domain.as_str()), ("count", &count.to_string())],
            );
            let _ = bot.send_message(message.chat.id, text).await;
        }
        Ok(None) => {
            let _ = bot
                .send_message(
                    message.chat.id,
                    translate(language, Key::CookieUploadReplay, &[]),
                )
                .await;
        }
        Err(yoink_downloads::DownloadError::InvalidInput) => {
            let _ = bot.send_message(message.chat.id, guidance()).await;
        }
        Err(error) => {
            let _ = bot
                .send_message(message.chat.id, download_error_response(error, language))
                .await;
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum FetchError {
    Invalid,
    Transport,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_cannot_escape_origin_or_use_url_syntax() {
        let base = file_base(
            &Url::parse("https://api.telegram.org/").unwrap(),
            "123:secret",
        );
        for path in [
            "/absolute",
            "../escape",
            "a/../b",
            "a//b",
            "a\\b",
            "a:b",
            "a?b",
            "a#b",
            "a%2fb",
            "",
            &"a".repeat(1025),
        ] {
            assert!(file_url(&base, path).is_none(), "{path}");
        }
        let url = file_url(&base, "documents/cookie.txt").unwrap();
        assert_eq!(url.origin(), base.origin());
        assert_eq!(url.path(), "/file/bot123:secret/documents/cookie.txt");
    }
    #[test]
    fn domain_from_filename_and_caption_is_strict() {
        assert_eq!(
            requested_upload(Some("/cookie@MYBOT"), Some("Example.COM.txt"), "mybot")
                .unwrap()
                .unwrap()
                .as_str(),
            "example.com"
        );
        assert!(
            requested_upload(Some("/cookie del example.com"), None, "bot")
                .unwrap()
                .is_err()
        );
        assert!(requested_upload(Some("/cookie@else"), None, "bot").is_none());
        assert!(requested_upload(Some("ordinary caption"), None, "bot").is_none());
        assert!(requested_upload(None, Some("example.com.txt"), "bot").is_none());
        assert_eq!(
            requested_upload(Some("/cookie"), Some("example.com.txt"), "bot")
                .unwrap()
                .unwrap()
                .as_str(),
            "example.com"
        );
        for reserved in ["del", "clear", "token", "upload"] {
            assert!(
                requested_upload(Some(&format!("/cookie {reserved}")), None, "bot")
                    .unwrap()
                    .is_err()
            );
        }
        assert!(
            requested_upload(Some("/cookie example.com extra"), None, "bot")
                .unwrap()
                .is_err()
        );
        assert!(filename_domain(Some("../example.com.txt")).is_none());
        assert!(filename_domain(Some("cookies")).is_none());
        assert!(filename_domain(Some("cookies.txt")).is_none());
    }
    #[test]
    fn config_requires_whole_seconds_in_range() {
        assert!(CookieUploadConfig::new(Duration::from_secs(1)).is_ok());
        assert!(CookieUploadConfig::new(Duration::from_secs(60)).is_ok());
        assert!(CookieUploadConfig::new(Duration::from_millis(1500)).is_err());
        assert!(CookieUploadConfig::new(Duration::from_secs(61)).is_err());
        let configured = CookieUploadConfig::new(Duration::from_secs(30))
            .unwrap()
            .with_local_files(
                PathBuf::from("/srv/bot-api/files"),
                PathBuf::from("/var/lib/app/files"),
            )
            .unwrap();
        let debug = format!("{configured:?}");
        assert!(!debug.contains("/srv/bot-api/files"));
        assert!(!debug.contains("/var/lib/app/files"));
        assert!(configured.clone().local_files.is_some());
        assert_eq!(
            CookieUploadConfig::default().timeout,
            Duration::from_secs(30)
        );
    }

    #[test]
    fn response_body_enforces_bounds_and_utf8() {
        assert!(cookie_text(b"# Netscape HTTP Cookie File\n".to_vec()).is_ok());
        assert!(cookie_text(Vec::new()).is_err());
        assert!(cookie_text(vec![0xff]).is_err());
        assert!(cookie_text(vec![b'x'; MAX_BYTES + 1]).is_err());
    }

    #[test]
    fn local_mapping_requires_exact_safe_prefix_and_is_opt_in() {
        for path in [
            "/srv/bot/files",
            "/srv/bot/files/../secret",
            "/srv/bot/files/a%2fb",
            "/srv/bot/files/a\\b",
            "/srv/bot/files//cookie.txt",
            "/srv/bot/files2/cookie.txt",
        ] {
            let cfg = CookieUploadConfig::new(Duration::from_secs(5)).unwrap();
            assert!(cfg.local_files.is_none());
            assert!(
                local_relative_path("/srv/bot/files", path).is_none(),
                "{path}"
            );
        }
        for root in [
            "/var/lib/telegram-bot-api/123:fixture-token/documents",
            "/var/lib/telegram-bot-api/123~fixture-token/documents",
        ] {
            assert!(
                CookieUploadConfig::default()
                    .with_local_files(PathBuf::from(root), PathBuf::from("/mnt/cookie-files"))
                    .is_ok()
            );
            assert_eq!(
                local_relative_path(root, &format!("{root}/file.txt")).as_deref(),
                Some("file.txt")
            );
        }
        assert_eq!(
            local_relative_path("/srv/bot/files", "/srv/bot/files/docs/cookie.txt").as_deref(),
            Some("docs/cookie.txt")
        );
        assert!(
            CookieUploadConfig::new(Duration::from_secs(5))
                .unwrap()
                .with_local_files(PathBuf::from("/srv/../files"), PathBuf::from("/tmp/root"))
                .is_err()
        );
        assert!(
            CookieUploadConfig::new(Duration::from_secs(5))
                .unwrap()
                .with_local_files(PathBuf::from("/srv/files"), PathBuf::from("/"))
                .is_err()
        );
    }

    #[tokio::test]
    async fn cancelled_local_read_keeps_its_slot_until_io_exits_and_then_recovers() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let slots = Arc::new(tokio::sync::Semaphore::new(1));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        {
            let first = bounded_local_read(slots.clone(), move || {
                let _ = started_tx.send(());
                release_rx.recv().map_err(|_| FetchError::Transport)?;
                Ok("finished".to_owned())
            });
            tokio::pin!(first);
            tokio::select! {
                result = &mut first => panic!("read returned before release: {result:?}"),
                started = started_rx => started.unwrap(),
            }
        }
        assert_eq!(slots.available_permits(), 0);
        let entered = Arc::new(AtomicBool::new(false));
        let entered_read = entered.clone();
        let second = tokio::time::timeout(
            Duration::from_millis(20),
            bounded_local_read(slots.clone(), move || {
                entered_read.store(true, Ordering::SeqCst);
                Ok("unexpected".to_owned())
            }),
        )
        .await;
        assert!(second.is_err());
        assert!(!entered.load(Ordering::SeqCst));
        release_tx.send(()).unwrap();
        let recovered = tokio::time::timeout(
            Duration::from_secs(2),
            bounded_local_read(slots, || Ok("recovered".to_owned())),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(recovered, "recovered");
    }

    #[test]
    fn stalled_local_read_does_not_hold_async_runtime_shutdown() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        runtime.block_on(async {
            let read = bounded_local_read(Arc::new(tokio::sync::Semaphore::new(1)), move || {
                let _ = started_tx.send(());
                release_rx.recv().map_err(|_| FetchError::Transport)?;
                Ok(String::new())
            });
            tokio::pin!(read);
            tokio::select! {
                result = &mut read => panic!("read returned before release: {result:?}"),
                started = started_rx => started.unwrap(),
            }
        });
        let (stopped_tx, stopped_rx) = std::sync::mpsc::channel();
        let shutdown = std::thread::spawn(move || {
            drop(runtime);
            let _ = stopped_tx.send(());
        });
        let stopped = stopped_rx.recv_timeout(Duration::from_secs(2));
        // Release even on regression, so a failed assertion cannot hang the suite.
        release_tx.send(()).unwrap();
        shutdown.join().unwrap();
        assert!(
            stopped.is_ok(),
            "runtime waited for a cancelled filesystem read"
        );
    }

    #[test]
    fn mapped_local_file_is_bounded_and_rejects_symlinks_and_nonfiles() {
        use std::{fs, os::unix::fs::symlink, time::SystemTime};
        let id = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("yoink-cookie-{id}"));
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("nested/valid.txt"), b"# Netscape cookies\n").unwrap();
        symlink("nested/valid.txt", root.join("leaf-link")).unwrap();
        symlink("nested", root.join("dir-link")).unwrap();
        let root_link = root.with_extension("link");
        symlink(&root, &root_link).unwrap();
        assert!(read_local_file(root.clone(), "nested/valid.txt".into()).is_ok());
        assert!(read_local_file(root_link.clone(), "nested/valid.txt".into()).is_err());
        assert!(read_local_file(root_link.join("nested"), "valid.txt".into()).is_err());
        rustix::fs::mknodat(
            rustix::fs::CWD,
            root.join("fifo"),
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            0,
        )
        .unwrap();
        assert!(read_local_file(root.clone(), "fifo".into()).is_err());
        assert!(read_local_file(root.clone(), "leaf-link".into()).is_err());
        assert!(read_local_file(root.clone(), "dir-link/valid.txt".into()).is_err());
        assert!(read_local_file(root.clone(), "nested".into()).is_err());
        fs::write(root.join("exact-limit"), vec![b'x'; MAX_BYTES]).unwrap();
        assert_eq!(
            read_local_file(root.clone(), "exact-limit".into())
                .unwrap()
                .len(),
            MAX_BYTES
        );
        fs::write(root.join("oversized"), vec![b'x'; MAX_BYTES + 1]).unwrap();
        assert!(read_local_file(root.clone(), "oversized".into()).is_err());
        fs::write(root.join("invalid-utf8"), [0xff]).unwrap();
        assert!(read_local_file(root.clone(), "invalid-utf8".into()).is_err());
        fs::remove_dir_all(root).unwrap();
        fs::remove_file(root_link).unwrap();
    }

    #[test]
    fn upload_responses_are_localized_and_contain_no_file_details() {
        for language in [yoink_i18n::Language::English, yoink_i18n::Language::Russian] {
            let guidance = crate::translate(language, Key::CookieUploadGuidance, &[]);
            let retry = crate::translate(language, Key::CookieUploadRetry, &[]);
            assert!(!guidance.is_empty());
            assert!(!retry.is_empty());
            assert!(!retry.contains("file_id"));
        }
    }
}
