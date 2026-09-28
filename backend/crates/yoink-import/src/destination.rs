use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use fs2::FileExt;
use tempfile::TempDir;

use crate::{ImportError, Result};

struct Reservation {
    file: File,
    path: PathBuf,
    keep: bool,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        // Never unlink after releasing the lock: another owner could acquire it
        // between unlock and unlink. A lost acquisition must also respect that owner.
        if !self.keep
            && self.file.try_lock_exclusive().is_ok()
            && let (Ok(owned), Ok(named)) = (self.file.metadata(), fs::symlink_metadata(&self.path))
            && (owned.dev(), owned.ino()) == (named.dev(), named.ino())
        {
            let _ = fs::remove_file(&self.path);
        }
        let _ = FileExt::unlock(&self.file);
    }
}

/// A private sibling staging directory and the final runtime ownership lock.
pub struct DestinationGuard {
    pub parent: PathBuf,
    pub target: PathBuf,
    pub staging: TempDir,
    parent_file: File,
    reservation: Reservation,
}

impl DestinationGuard {
    pub fn reserve(target: &Path) -> Result<Self> {
        if target.as_os_str().is_empty() {
            return Err(ImportError::Destination);
        }
        let raw_parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let original = fs::symlink_metadata(raw_parent).map_err(|_| ImportError::Destination)?;
        if !original.is_dir()
            || original.file_type().is_symlink()
            || original.uid() != rustix::process::geteuid().as_raw()
            || original.permissions().mode() & 0o077 != 0
            || original.permissions().mode() & 0o300 != 0o300
        {
            return Err(ImportError::Destination);
        }
        let parent = fs::canonicalize(raw_parent).map_err(|_| ImportError::Destination)?;
        let parent_file = directory(&parent)?;
        let opened = parent_file
            .metadata()
            .map_err(|_| ImportError::Destination)?;
        if (original.dev(), original.ino()) != (opened.dev(), opened.ino()) {
            return Err(ImportError::Destination);
        }
        let filename = target.file_name().ok_or(ImportError::Destination)?;
        let target = parent.join(filename);
        for suffix in ["", ".yoink-lock", "-wal", "-shm", "-journal"] {
            ensure_absent(&suffixed(&target, suffix))?;
        }
        let lock_path = suffixed(&target, ".yoink-lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&lock_path)
            .map_err(|_| ImportError::Destination)?;
        let reservation = Reservation {
            file,
            path: lock_path,
            keep: false,
        };
        reservation
            .file
            .try_lock_exclusive()
            .map_err(|_| ImportError::Destination)?;
        if reservation
            .file
            .metadata()
            .map_err(|_| ImportError::Destination)?
            .nlink()
            != 1
        {
            return Err(ImportError::Destination);
        }
        let staging = tempfile::Builder::new()
            .prefix(".yoink-import-")
            .tempdir_in(&parent)
            .map_err(|_| ImportError::Destination)?;
        fs::set_permissions(staging.path(), fs::Permissions::from_mode(0o700))
            .map_err(|_| ImportError::Destination)?;
        let result = Self {
            parent,
            target,
            staging,
            parent_file,
            reservation,
        };
        result.verify_parent()?;
        Ok(result)
    }

    pub fn staging_db(&self) -> PathBuf {
        self.staging.path().join("database.db")
    }

    fn verify_parent(&self) -> Result<()> {
        let named = fs::symlink_metadata(&self.parent).map_err(|_| ImportError::Destination)?;
        let held = self
            .parent_file
            .metadata()
            .map_err(|_| ImportError::Destination)?;
        if !named.is_dir()
            || named.file_type().is_symlink()
            || (named.dev(), named.ino()) != (held.dev(), held.ino())
            || named.permissions().mode() & 0o077 != 0
            || named.uid() != rustix::process::geteuid().as_raw()
        {
            return Err(ImportError::Destination);
        }
        Ok(())
    }

    /// The caller has validated and closed the native engine before this boundary.
    pub fn publish(&mut self, staged: &Path) -> Result<()> {
        self.verify_parent()?;
        if staged != self.staging_db() {
            return Err(ImportError::Destination);
        }
        for suffix in ["", "-wal", "-shm", "-journal"] {
            ensure_absent(&suffixed(&self.target, suffix))?;
        }
        let held = self
            .reservation
            .file
            .metadata()
            .map_err(|_| ImportError::Destination)?;
        let named =
            fs::symlink_metadata(&self.reservation.path).map_err(|_| ImportError::Destination)?;
        if (held.dev(), held.ino()) != (named.dev(), named.ino()) || held.nlink() != 1 {
            return Err(ImportError::Destination);
        }
        let metadata = fs::symlink_metadata(staged).map_err(|_| ImportError::Storage)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.nlink() != 1
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(ImportError::Destination);
        }
        File::open(staged)
            .and_then(|file| file.sync_all())
            .map_err(|_| ImportError::Storage)?;
        let staging_directory = directory(self.staging.path())?;
        #[cfg(target_os = "linux")]
        {
            rustix::fs::renameat_with(
                &staging_directory,
                "database.db",
                &self.parent_file,
                self.target.file_name().ok_or(ImportError::Destination)?,
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(|_| ImportError::Storage)?;
            // From this point an error must leave the published database intact.
            self.reservation.keep = true;
            self.parent_file
                .sync_all()
                .map_err(|_| ImportError::PublishedDurabilityUncertain)?;
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = staging_directory;
            Err(ImportError::EngineCapability)
        }
    }
}

fn directory(path: &Path) -> Result<File> {
    rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| ImportError::Destination)
}

fn ensure_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(ImportError::Destination),
    }
}
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn private_dir() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    #[test]
    fn rejects_insecure_parent_and_each_existing_artifact() {
        let dir = private_dir();
        let target = dir.path().join("db");
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(DestinationGuard::reserve(&target).is_err());
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        for suffix in ["", ".yoink-lock", "-wal", "-shm", "-journal"] {
            let artifact = suffixed(&target, suffix);
            fs::write(&artifact, b"sentinel").unwrap();
            assert!(DestinationGuard::reserve(&target).is_err());
            assert_eq!(fs::read(&artifact).unwrap(), b"sentinel");
            fs::remove_file(artifact).unwrap();
        }
    }

    #[test]
    fn failed_import_removes_only_its_reservation_and_staging() {
        let dir = private_dir();
        let target = dir.path().join("db");
        let staging;
        {
            let guard = DestinationGuard::reserve(&target).unwrap();
            staging = guard.staging.path().to_owned();
            assert!(DestinationGuard::reserve(&target).is_err());
            assert_eq!(
                fs::metadata(suffixed(&target, ".yoink-lock"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert!(!staging.exists());
        assert!(!suffixed(&target, ".yoink-lock").exists());
        assert!(!target.exists());
    }

    #[test]
    fn publication_never_clobbers_a_late_destination() {
        let dir = private_dir();
        let target = dir.path().join("db");
        let mut guard = DestinationGuard::reserve(&target).unwrap();
        let staged = guard.staging_db();
        fs::write(&staged, b"candidate").unwrap();
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&target, b"concurrent").unwrap();
        assert!(guard.publish(&staged).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"concurrent");
    }

    #[test]
    fn successful_publication_retains_runtime_lock_name() {
        let dir = private_dir();
        let target = dir.path().join("db");
        {
            let mut guard = DestinationGuard::reserve(&target).unwrap();
            let staged = guard.staging_db();
            fs::write(&staged, b"complete").unwrap();
            fs::set_permissions(&staged, fs::Permissions::from_mode(0o600)).unwrap();
            guard.publish(&staged).unwrap();
        }
        assert_eq!(fs::read(&target).unwrap(), b"complete");
        let lock = File::open(suffixed(&target, ".yoink-lock")).unwrap();
        lock.try_lock_exclusive().unwrap();
    }
}
