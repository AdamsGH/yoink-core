#![forbid(unsafe_code)]

use std::{
    env,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
};

use landlock::{
    ABI, Access, AccessFs, CompatLevel, Compatible, Ruleset, RulesetAttr, RulesetCreatedAttr,
    RulesetStatus, Scope, path_beneath_rules,
};

const ERROR: &str = "worker launcher failed";

const WORKER_ENV: [&str; 7] = [
    "YOINK_WORKER_MAX_BYTES",
    "YOINK_WORKER_SOCKET_TIMEOUT_SECS",
    "YOINK_WORKER_PROCESSING_TIMEOUT_SECS",
    "YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS",
    "YOINK_WORKER_ALLOWED_PROXY_ORIGINS",
    "YOINK_WORKER_MAX_ITEMS",
    "YOINK_WORKER_TOTAL_MAX_BYTES",
];

fn main() {
    if run().is_err() {
        eprintln!("{ERROR}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), ()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let (check_only, program, program_args) = match args.as_slice() {
        [] => (
            false,
            PathBuf::from("/usr/local/bin/python"),
            vec!["-I".into(), "-B".into(), "/opt/yoink-worker/main.py".into()],
        ),
        [check] if check == "--check" => (true, PathBuf::new(), Vec::new()),
        [separator, program, rest @ ..] if separator == "--" => {
            let program = PathBuf::from(program);
            if !program.is_absolute() {
                return Err(());
            }
            (false, program, rest.to_vec())
        }
        _ => return Err(()),
    };

    let cwd = env::current_dir()
        .map_err(|_| ())?
        .canonicalize()
        .map_err(|_| ())?;
    validate_cwd(&cwd)?;
    let allowed_env: Vec<_> = WORKER_ENV
        .iter()
        .filter_map(|key| env::var_os(key).map(|value| ((*key).to_owned(), value)))
        .collect();

    apply_confinement(&cwd)?;

    if check_only {
        return Ok(());
    }

    let mut command = Command::new(program);
    command
        .args(program_args)
        .current_dir(&cwd)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .env("HOME", &cwd)
        .env("TMPDIR", &cwd)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .envs(allowed_env);
    let _error = command.exec();
    Err(())
}

fn validate_cwd(cwd: &Path) -> Result<(), ()> {
    if !cwd.is_absolute() || cwd == Path::new("/") {
        return Err(());
    }

    let metadata = cwd.metadata().map_err(|_| ())?;
    if metadata.permissions().mode() & 0o7777 != 0o700 {
        return Err(());
    }

    const FORBIDDEN: [&str; 10] = [
        "/usr",
        "/lib",
        "/lib64",
        "/opt/yoink-worker",
        "/proc",
        "/sys",
        "/dev",
        "/etc",
        "/root",
        "/home",
    ];
    if FORBIDDEN.iter().any(|path| cwd.starts_with(path)) {
        return Err(());
    }
    Ok(())
}

fn apply_confinement(cwd: &Path) -> Result<(), ()> {
    let abi = ABI::V6;
    let mut readonly = vec![
        PathBuf::from("/usr"),
        PathBuf::from("/lib"),
        PathBuf::from("/lib64"),
        PathBuf::from("/opt/yoink-worker"),
        PathBuf::from("/etc/ssl"),
        PathBuf::from("/dev/urandom"),
    ];
    readonly.extend(
        [
            "/etc/hosts",
            "/etc/resolv.conf",
            "/etc/nsswitch.conf",
            "/etc/ld.so.cache",
            "/etc/gai.conf",
        ]
        .iter()
        .map(PathBuf::from),
    );
    readonly.retain(|path| path.exists());

    let read = AccessFs::from_read(abi);
    let mut writable_devices = vec![PathBuf::from("/dev/null")];
    writable_devices.retain(|path| path.exists());
    let mut rules: Vec<_> = path_beneath_rules(&readonly, read).collect();
    rules.extend(path_beneath_rules(
        &writable_devices,
        AccessFs::from_read(abi) | AccessFs::from_write(abi),
    ));
    rules.extend(path_beneath_rules(
        &[cwd.to_path_buf()],
        AccessFs::from_all(abi),
    ));

    let status = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(abi))
        .map_err(|_| ())?
        .scope(Scope::Signal | Scope::AbstractUnixSocket)
        .map_err(|_| ())?
        .create()
        .map_err(|_| ())?
        .add_rules(rules)
        .map_err(|_| ())?
        .restrict_self()
        .map_err(|_| ())?;
    if status.ruleset != RulesetStatus::FullyEnforced || !status.no_new_privs {
        return Err(());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::WORKER_ENV;

    #[test]
    fn worker_environment_allowlist_contains_typed_processing_limit() {
        assert!(WORKER_ENV.contains(&"YOINK_WORKER_PROCESSING_TIMEOUT_SECS"));
        assert_eq!(WORKER_ENV.len(), 7);
        assert!(!WORKER_ENV.contains(&"DATABASE_URL"));
        assert!(!WORKER_ENV.contains(&"TELEGRAM_BOT_TOKEN"));
    }
}
