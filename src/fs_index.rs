use jwalk::WalkDir;
use ms2cc::Ms2ccError;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub struct FileWalker {
    inner: Box<dyn Iterator<Item = Result<PathBuf, Ms2ccError>> + Send>,
}

impl FileWalker {
    pub fn new_with_threads(
        root: PathBuf,
        exclude_directories: Vec<String>,
        file_extensions: Vec<String>,
        num_threads: usize,
    ) -> Self {
        let exclude_directories: Vec<String> = exclude_directories
            .into_iter()
            .map(|value| value.to_lowercase())
            .collect();
        let allowed_extensions: Vec<String> = file_extensions
            .into_iter()
            .map(|value| value.to_lowercase())
            .collect();

        let exclude_dirs_clone = exclude_directories.clone();
        let allowed_exts_clone = allowed_extensions.clone();

        let walker = WalkDir::new(root)
            .parallelism(jwalk::Parallelism::RayonNewPool(num_threads))
            .skip_hidden(false)
            .process_read_dir(move |_depth, _path, _state, children| {
                children.retain(|entry_result| {
                    if let Ok(entry) = entry_result {
                        let path = entry.path();
                        let file_type = entry.file_type();

                        if file_type.is_dir() {
                            !should_skip_directory(&path, &exclude_dirs_clone)
                        } else if file_type.is_file() {
                            is_allowed_file(&path, &allowed_exts_clone)
                        } else {
                            false
                        }
                    } else {
                        true
                    }
                });
            });

        let iter =
            walker.into_iter().filter_map(
                move |entry_result| match entry_result {
                    Ok(entry) => {
                        let path = entry.path();
                        if entry.file_type().is_file() {
                            Some(normalize_path(&path))
                        } else {
                            None
                        }
                    }
                    Err(err) => {
                        let path = err
                            .path()
                            .unwrap_or_else(|| Path::new(""))
                            .to_path_buf();
                        Some(Err(Ms2ccError::io_error(err.into(), path)))
                    }
                },
            );

        Self {
            inner: Box::new(iter),
        }
    }
}

fn should_skip_directory(path: &Path, exclude_directories: &[String]) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(|name| name.to_lowercase())
        .map(|name| exclude_directories.iter().any(|value| value == &name))
        .unwrap_or(false)
}

fn is_allowed_file(path: &Path, allowed_extensions: &[String]) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|ext| ext.to_lowercase())
        .map(|ext| allowed_extensions.iter().any(|value| value == &ext))
        .unwrap_or(false)
}

fn normalize_path(path: &Path) -> Result<PathBuf, Ms2ccError> {
    match path.to_str() {
        Some(value) => Ok(PathBuf::from(value.to_lowercase())),
        None => Err(Ms2ccError::PathNormalization {
            path: path.to_path_buf(),
        }),
    }
}

impl Iterator for FileWalker {
    type Item = Result<PathBuf, Ms2ccError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

#[cfg(test)]
mod tests {
    use super::FileWalker;
    use ms2cc::Ms2ccError;
    use std::fs::{File, create_dir_all};
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn touch_file(path: &PathBuf) {
        let mut file = File::create(path).expect("create file");
        writeln!(file, "test").expect("write file");
    }

    #[test]
    fn walker_skips_excluded_directories_and_filters_extensions() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let root = temp_dir.path();

        let include_dir = root.join("src");
        let exclude_dir = root.join(".git");
        create_dir_all(&include_dir).expect("create include dir");
        create_dir_all(&exclude_dir).expect("create exclude dir");

        let allowed_file = include_dir.join("main.cpp");
        let skipped_ext = include_dir.join("readme.md");
        let hidden_allowed = exclude_dir.join("ignored.cpp");

        touch_file(&allowed_file);
        touch_file(&skipped_ext);
        touch_file(&hidden_allowed);

        let walker = FileWalker::new_with_threads(
            root.to_path_buf(),
            vec![".git".to_string()],
            vec!["cpp".to_string()],
            1,
        );

        let collected: Result<Vec<PathBuf>, Ms2ccError> = walker.collect();
        let files = collected.expect("walker success");

        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0],
            PathBuf::from(allowed_file.to_str().unwrap().to_lowercase())
        );
    }

    #[test]
    fn walker_normalizes_paths_to_lowercase() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let root = temp_dir.path();
        let include_dir = root.join("SRC");
        create_dir_all(&include_dir).expect("create include dir");
        let mixed_case = include_dir.join("Main.CPP");
        touch_file(&mixed_case);

        let walker = FileWalker::new_with_threads(
            root.to_path_buf(),
            Vec::new(),
            vec!["cpp".to_string()],
            1,
        );

        let files: Vec<PathBuf> = walker
            .collect::<Result<_, Ms2ccError>>()
            .expect("walker success");

        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0],
            PathBuf::from(mixed_case.to_str().unwrap().to_lowercase())
        );
    }
}
