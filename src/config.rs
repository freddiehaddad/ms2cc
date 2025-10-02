//! Shared configuration definitions used by both the library and the binary.

/// Default directories to exclude during traversal.
pub const DEFAULT_EXCLUDE_DIRECTORIES: &[&str] = &[".git"];

/// Default file extensions that should be processed.
pub const DEFAULT_FILE_EXTENSIONS: &[&str] = &[
    "c", "cc", "cpp", "cxx", "c++", "h", "hh", "hpp", "hxx", "h++", "inl",
];

/// Default compiler executable to search for in logs.
pub const DEFAULT_COMPILER_EXECUTABLE: &str = "cl.exe";

/// Default number of worker threads per task.
pub const DEFAULT_MAX_THREADS: usize = 8;

/// Configuration for the ms2cc tool.
#[derive(Debug, Clone)]
pub struct Config {
    pub exclude_directories: Vec<String>,
    pub file_extensions: Vec<String>,
    pub compiler_executable: String,
}

impl Default for Config {
    /// Provides sensible defaults that mirror the command-line flags exposed by
    /// the binary so tests and library consumers share the same baseline.
    fn default() -> Self {
        Self {
            exclude_directories: default_exclude_directories(),
            file_extensions: default_file_extensions(),
            compiler_executable: DEFAULT_COMPILER_EXECUTABLE.to_string(),
        }
    }
}

impl Config {
    /// Returns the default exclude directories as owned strings.
    pub fn default_exclude_directories() -> Vec<String> {
        default_exclude_directories()
    }

    /// Returns the default file extensions as owned strings.
    pub fn default_file_extensions() -> Vec<String> {
        default_file_extensions()
    }
}

fn default_exclude_directories() -> Vec<String> {
    DEFAULT_EXCLUDE_DIRECTORIES
        .iter()
        .map(|entry| (*entry).to_string())
        .collect()
}

fn default_file_extensions() -> Vec<String> {
    DEFAULT_FILE_EXTENSIONS
        .iter()
        .map(|entry| (*entry).to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_constants() {
        let config = Config::default();
        assert_eq!(config.exclude_directories, default_exclude_directories());
        assert_eq!(config.file_extensions, default_file_extensions());
        assert_eq!(config.compiler_executable, DEFAULT_COMPILER_EXECUTABLE);
    }

    #[test]
    fn default_lists_include_expected_entries() {
        let excludes = default_exclude_directories();
        assert!(excludes.iter().any(|value| value == ".git"));

        let extensions = default_file_extensions();
        for ext in ["c", "cpp", "h", "hpp"] {
            assert!(extensions.iter().any(|value| value == ext));
        }
    }
}
