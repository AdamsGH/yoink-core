use std::fs::Metadata;
use std::path::Path;

use crate::Error;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl FileIdentity {
    pub(crate) fn from_metadata(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            Self {}
        }
    }

    pub(crate) fn read(path: &Path) -> Result<Self, Error> {
        let metadata = std::fs::symlink_metadata(path).map_err(|_| Error::InvalidPath)?;
        if !metadata.file_type().is_file() {
            return Err(Error::InvalidPath);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(Error::InvalidPath);
            }
        }
        Ok(Self::from_metadata(&metadata))
    }
}

pub(crate) fn verify(path: &Path, database: FileIdentity, lock: FileIdentity) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let parent = path.parent().ok_or(Error::InvalidPath)?;
        let metadata = std::fs::metadata(parent).map_err(|_| Error::InvalidPath)?;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::InsecureDirectory);
        }
    }
    if FileIdentity::read(path)? != database
        || FileIdentity::read(&crate::sidecar_path(path))? != lock
    {
        return Err(Error::InvalidPath);
    }
    Ok(())
}
