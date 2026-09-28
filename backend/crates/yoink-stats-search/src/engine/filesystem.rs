use std::{
    fs,
    path::{Component, Path, PathBuf},
};

pub(super) fn secure_directory(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() || path.file_name().is_none() {
        return Err("index directory path is invalid".into());
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(index_error)?.join(path)
    };
    let mut prefix = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::RootDir | Component::Normal(_) => {
                prefix.push(component.as_os_str());
                match fs::symlink_metadata(&prefix) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        return Err("index path must not traverse symlinks".into());
                    }
                    Ok(_) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(index_error(e)),
                }
            }
            Component::CurDir => (),
            Component::ParentDir | Component::Prefix(_) => {
                return Err("index path contains an unsafe component".into());
            }
        }
    }
    let parent = absolute
        .parent()
        .ok_or_else(|| "index parent is invalid".to_owned())?;
    let parent_meta = fs::symlink_metadata(parent)
        .map_err(|e| format!("index parent must exist and be private: {e}"))?;
    if !parent_meta.is_dir() || parent_meta.file_type().is_symlink() {
        return Err("index parent must be a real directory".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if parent_meta.permissions().mode() & 0o077 != 0 {
            return Err("index parent directory must be private".into());
        }
    }
    match fs::symlink_metadata(&absolute) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if meta.permissions().mode() & 0o077 != 0 {
                    return Err("existing index directory must be private".into());
                }
            }
            validate_entries(&absolute)?;
        }
        Ok(_) => return Err("index path must be a real directory".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&absolute).map_err(index_error)?;
        }
        Err(e) => return Err(index_error(e)),
    }
    Ok(())
}

fn validate_entries(dir: &Path) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(index_error)? {
        let entry = entry.map_err(index_error)?;
        let meta = fs::symlink_metadata(entry.path()).map_err(index_error)?;
        if meta.file_type().is_symlink() {
            return Err("index directory contains a symlink; use a fresh private directory".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.is_file() && meta.nlink() != 1 {
                return Err(
                    "index directory contains a hard-linked file; use a fresh private directory"
                        .into(),
                );
            }
        }
        if meta.is_dir() {
            validate_entries(&entry.path())?;
        } else if !meta.is_file() {
            return Err("index directory contains an unsupported entry".into());
        }
    }
    Ok(())
}

fn index_error(error: impl std::fmt::Display) -> String {
    format!("cannot safely access Tantivy index directory: {error}")
}
