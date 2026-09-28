use std::{
    fs::{File, Metadata, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::{Error, Report, Result, check_cancel};

const MAGIC: &[u8; 16] = b"YOINK-BACKUP-v1\0";
const HEADER: u64 = 56;

fn input(path: &Path) -> Result<(File, Metadata)> {
    let fd = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| Error::Source)?;
    let file = File::from(fd);
    let meta = file.metadata().map_err(|_| Error::Source)?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.permissions().mode() & 0o077 != 0
    {
        return Err(Error::Source);
    }
    Ok((file, meta))
}

fn output(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| Error::Storage)
}

fn unchanged(file: &File, before: &Metadata) -> Result<()> {
    let after = file.metadata().map_err(|_| Error::Source)?;
    if before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
        || after.nlink() != 1
    {
        return Err(Error::Source);
    }
    Ok(())
}

fn transfer(
    input: &mut File,
    output: &mut impl Write,
    bytes: u64,
    cancel: &CancellationToken,
) -> Result<[u8; 32]> {
    let mut buffer = [0; 64 * 1024];
    let mut hash = Sha256::new();
    let mut remaining = bytes;
    while remaining != 0 {
        check_cancel(cancel)?;
        let size = remaining.min(buffer.len() as u64) as usize;
        input
            .read_exact(&mut buffer[..size])
            .map_err(|_| Error::Archive)?;
        output
            .write_all(&buffer[..size])
            .map_err(|_| Error::Storage)?;
        hash.update(&buffer[..size]);
        remaining -= size as u64;
    }
    let mut tail = [0];
    if input.read(&mut tail).map_err(|_| Error::Archive)? != 0 {
        return Err(Error::Archive);
    }
    Ok(hash.finalize().into())
}

pub(crate) fn encode(
    snapshot: &Path,
    destination: &Path,
    max_bytes: u64,
    cancel: &CancellationToken,
) -> Result<Report> {
    check_cancel(cancel)?;
    let (mut source, before) = input(snapshot)?;
    let bytes = before.len();
    if bytes == 0 || bytes > max_bytes {
        return Err(Error::Archive);
    }
    let mut dest = output(destination)?;
    dest.write_all(&[0; HEADER as usize])
        .map_err(|_| Error::Storage)?;
    let digest = transfer(&mut source, &mut dest, bytes, cancel)?;
    unchanged(&source, &before)?;
    check_cancel(cancel)?;
    dest.seek(SeekFrom::Start(0)).map_err(|_| Error::Storage)?;
    dest.write_all(MAGIC)
        .and_then(|_| dest.write_all(&bytes.to_be_bytes()))
        .and_then(|_| dest.write_all(&digest))
        .and_then(|_| dest.sync_all())
        .map_err(|_| Error::Storage)?;
    Ok(Report {
        bytes,
        sha256: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
    })
}

pub(crate) fn decode(
    source: &Path,
    destination: &Path,
    max_bytes: u64,
    cancel: &CancellationToken,
) -> Result<Report> {
    check_cancel(cancel)?;
    let (mut source, before) = input(source)?;
    let mut header = [0; HEADER as usize];
    source.read_exact(&mut header).map_err(|_| Error::Archive)?;
    if &header[..16] != MAGIC {
        return Err(Error::Archive);
    }
    let bytes = u64::from_be_bytes(header[16..24].try_into().map_err(|_| Error::Archive)?);
    if bytes == 0 || bytes > max_bytes || bytes.checked_add(HEADER) != Some(before.len()) {
        return Err(Error::Archive);
    }
    let mut dest = output(destination)?;
    let digest = transfer(&mut source, &mut dest, bytes, cancel)?;
    if digest.as_slice() != &header[24..] {
        return Err(Error::Archive);
    }
    unchanged(&source, &before)?;
    check_cancel(cancel)?;
    dest.sync_all().map_err(|_| Error::Storage)?;
    Ok(Report {
        bytes,
        sha256: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn private(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[test]
    fn envelope_validates_bytes_and_rejects_corruption_without_clobbering() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let packed = dir.path().join("packed");
        let out = dir.path().join("out");
        let cancel = CancellationToken::new();
        private(&source, &vec![42; 150_000]);
        let report = encode(&source, &packed, 200_000, &cancel).unwrap();
        assert_eq!(decode(&packed, &out, 200_000, &cancel).unwrap(), report);
        assert_eq!(
            std::fs::read(&source).unwrap(),
            std::fs::read(&out).unwrap()
        );
        assert!(decode(&packed, &out, 200_000, &cancel).is_err());
        let good = std::fs::read(&packed).unwrap();
        for i in [0, 16, 24, 55, 56, good.len() - 1] {
            let mut corrupt = good.clone();
            corrupt[i] ^= 1;
            private(&packed, &corrupt);
            assert!(
                decode(
                    &packed,
                    &dir.path().join(format!("bad-{i}")),
                    200_000,
                    &cancel
                )
                .is_err()
            );
        }
        for (name, data) in [
            ("truncated", good[..good.len() - 1].to_vec()),
            ("appended", [good.clone(), vec![1]].concat()),
        ] {
            private(&packed, &data);
            assert!(decode(&packed, &dir.path().join(name), 200_000, &cancel).is_err());
        }
        private(&packed, &good);
        assert!(decode(&packed, &dir.path().join("limit"), 100, &cancel).is_err());
        cancel.cancel();
        assert!(matches!(
            decode(&packed, &dir.path().join("cancel"), 200_000, &cancel),
            Err(Error::Cancelled)
        ));
        assert!(!dir.path().join("cancel").exists());
    }
    #[test]
    fn rejects_unsafe_source_and_propagates_write_failure() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let out = dir.path().join("out");
        let link = dir.path().join("link");
        private(&source, b"secret");
        let cancel = CancellationToken::new();
        std::os::unix::fs::symlink(&source, &link).unwrap();
        assert!(matches!(
            encode(&link, &out, 100, &cancel),
            Err(Error::Source)
        ));
        std::fs::remove_file(&link).unwrap();
        std::fs::hard_link(&source, &link).unwrap();
        assert!(matches!(
            encode(&source, &out, 100, &cancel),
            Err(Error::Source)
        ));
        std::fs::remove_file(&link).unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            encode(&source, &out, 100, &cancel),
            Err(Error::Source)
        ));
        let mut input = File::open(&source).unwrap();
        let mut full = OpenOptions::new().write(true).open("/dev/full").unwrap();
        assert!(matches!(
            transfer(&mut input, &mut full, 6, &cancel),
            Err(Error::Storage)
        ));
    }
}
