use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

struct ProbeProcess(std::process::Child);

impl Drop for ProbeProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn launcher() -> &'static str {
    env!("CARGO_BIN_EXE_yoink-worker-launcher")
}

fn private_dir(path: &Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn confined_worker_access_and_environment() {
    let parent = tempfile::tempdir().unwrap();
    let cwd = parent.path().join("attempt");
    private_dir(&cwd);
    let outside = parent.path().join("outside-secret");
    fs::write(&outside, "outside").unwrap();
    let probe = ProbeProcess(
        Command::new("/bin/sleep")
            .arg("30")
            .env_clear()
            .spawn()
            .unwrap(),
    );
    let runtime_file = Path::new("/usr/bin/env");
    assert!(runtime_file.exists());

    let script = format!(
        "set -eu
printf owned > own-file
test \"$(cat own-file)\" = owned
cat {} > /dev/null
if printf denied > {}; then exit 21; fi
if cat {}; then exit 22; fi
if cat /proc/{}/environ; then exit 23; fi
if kill -0 {}; then exit 24; fi
test \"$HOME\" = \"$PWD\"
test \"$TMPDIR\" = \"$PWD\"
test \"$PYTHONDONTWRITEBYTECODE\" = 1
test \"$PATH\" = /usr/local/bin:/usr/bin:/bin
test \"$LANG\" = C.UTF-8
test \"$YOINK_WORKER_MAX_BYTES\" = 123
test -z \"${{SENTINEL:-}}\"",
        runtime_file.display(),
        outside.display(),
        outside.display(),
        probe.0.id(),
        probe.0.id(),
    );
    let output = Command::new(launcher())
        .arg("--")
        .arg("/bin/sh")
        .arg("-c")
        .arg(script)
        .current_dir(&cwd)
        .env("SENTINEL", "must-not-leak")
        .env("YOINK_WORKER_MAX_BYTES", "123")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "launcher failed with status {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(cwd.join("own-file")).unwrap(), b"owned");
}

#[test]
fn check_mode_applies_confinement_and_rejects_non_private_cwd() {
    let parent = tempfile::tempdir().unwrap();
    let good = parent.path().join("good");
    private_dir(&good);
    let status = Command::new(launcher())
        .arg("--check")
        .current_dir(&good)
        .status()
        .unwrap();
    assert!(status.success());

    let bad = parent.path().join("bad");
    fs::create_dir(&bad).unwrap();
    let output = Command::new(launcher())
        .arg("--check")
        .current_dir(&bad)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(output.stderr, b"worker launcher failed\n");
}
