use std::path::{Component, Path, PathBuf};

pub fn absolute_lexical(path: &Path, base: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    clean(&joined)
}

pub fn clean(path: &Path) -> PathBuf {
    let mut prefix = None;
    let mut has_root = false;
    let mut parts = Vec::new();

    for component in path.components() {
        match component {
            Component::Prefix(value) => prefix = Some(value.as_os_str().to_os_string()),
            Component::RootDir => has_root = true,
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.last().is_some_and(|part| part != "..") {
                    parts.pop();
                } else if !has_root {
                    parts.push("..".into());
                }
            }
            Component::Normal(value) => parts.push(value.to_os_string()),
        }
    }

    let mut result = PathBuf::new();
    if let Some(value) = prefix {
        result.push(value);
    }
    if has_root {
        result.push("\\");
    }
    for part in parts {
        result.push(part);
    }
    result
}

pub fn display(path: &Path) -> String {
    let value = clean(path).to_string_lossy().replace('/', "\\");
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = value.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        value
    }
}

pub fn key(path: &Path) -> String {
    display(path).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_windows_parent_and_current_components() {
        assert_eq!(
            display(Path::new(r"C:\work\src\.\sub\..\main.cpp")),
            r"C:\work\src\main.cpp"
        );
    }

    #[test]
    fn removes_verbatim_windows_prefixes() {
        assert_eq!(
            display(Path::new(r"\\?\C:\work\main.cpp")),
            r"C:\work\main.cpp"
        );
        assert_eq!(
            display(Path::new(r"\\?\UNC\server\share\main.cpp")),
            r"\\server\share\main.cpp"
        );
    }
}
