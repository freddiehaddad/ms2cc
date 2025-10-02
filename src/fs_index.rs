use ms2cc::Ms2ccError;
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fs::read_dir;
use std::path::{Path, PathBuf};

/// Iterator that walks a directory tree and yields normalized file paths that
/// match the configured filters.
pub struct FileWalker {
    queue: VecDeque<PathBuf>,
    exclude_directories: Vec<String>,
    allowed_extensions: Vec<String>,
}

impl FileWalker {
    /// Creates a new iterator rooted at `root`, applying the provided exclusion
    /// and extension filters.
    pub fn new(
        root: PathBuf,
        exclude_directories: Vec<String>,
        file_extensions: Vec<String>,
    ) -> Self {
        let exclude_directories = exclude_directories
            .into_iter()
            .map(|value| value.to_lowercase())
            .collect();
        let allowed_extensions = file_extensions
            .into_iter()
            .map(|value| value.to_lowercase())
            .collect();

        let mut queue = VecDeque::new();
        queue.push_back(root);

        Self {
            queue,
            exclude_directories,
            allowed_extensions,
        }
    }

    fn should_skip_directory(&self, path: &Path) -> bool {
        path.file_name()
            .and_then(OsStr::to_str)
            .map(|name| name.to_lowercase())
            .map(|name| {
                self.exclude_directories.iter().any(|value| value == &name)
            })
            .unwrap_or(false)
    }

    fn is_allowed_file(&self, path: &Path) -> bool {
        path.extension()
            .and_then(OsStr::to_str)
            .map(|ext| ext.to_lowercase())
            .map(|ext| {
                self.allowed_extensions.iter().any(|value| value == &ext)
            })
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
}

impl Iterator for FileWalker {
    type Item = Result<PathBuf, Ms2ccError>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(path) = self.queue.pop_front() {
            if path.is_dir() {
                if self.should_skip_directory(&path) {
                    continue;
                }

                match read_dir(&path) {
                    Ok(entries) => {
                        for entry in entries {
                            match entry {
                                Ok(dir_entry) => {
                                    let entry_path = dir_entry.path();
                                    if entry_path.is_dir() {
                                        self.queue.push_back(entry_path);
                                    } else if entry_path.is_file() {
                                        if self.is_allowed_file(&entry_path) {
                                            self.queue.push_back(entry_path);
                                        }
                                    } else {
                                        return Some(Err(
                                            Ms2ccError::UnexpectedEntry {
                                                path: entry_path,
                                            },
                                        ));
                                    }
                                }
                                Err(err) => {
                                    return Some(Err(Ms2ccError::io_error(
                                        err,
                                        path.clone(),
                                    )));
                                }
                            }
                        }
                        continue;
                    }
                    Err(err) => {
                        return Some(Err(Ms2ccError::io_error(
                            err,
                            path.clone(),
                        )));
                    }
                }
            } else if path.is_file() {
                if self.is_allowed_file(&path) {
                    return Some(Self::normalize_path(&path));
                }
            } else {
                return Some(Err(Ms2ccError::UnexpectedEntry { path }));
            }
        }

        None
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

        let walker = FileWalker::new(
            root.to_path_buf(),
            vec![".git".to_string()],
            vec!["cpp".to_string()],
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

        let walker = FileWalker::new(
            root.to_path_buf(),
            Vec::new(),
            vec!["cpp".to_string()],
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
