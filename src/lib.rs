//! Core library for transforming MSBuild logs into `compile_commands.json`.
//!
//! The crate exposes parsing utilities, compile command helpers, shared data
//! structures, and configuration types that power both the binary and tests.

use serde::{Deserialize, Serialize};
use std::ffi::OsString;

// Ensures that compile commands serialize to JSON and back without losing
// data, validating the serde helpers used by downstream tooling.
#[test]
fn test_compile_command_serialization_roundtrip() {
    let command = CompileCommand {
        file: PathBuf::from("file.cpp"),
        directory: PathBuf::from("/tmp/project"),
        arguments: vec![
            OsString::from("cl.exe"),
            OsString::from("/c"),
            OsString::from("file.cpp"),
        ],
    };

    let json = serde_json::to_string(&command).expect("serialize");
    let rebuilt: CompileCommand =
        serde_json::from_str(&json).expect("deserialize");

    assert_eq!(rebuilt, command);
}
use std::path::PathBuf;

pub mod error;
pub use error::Ms2ccError;
pub mod config;
pub use config::Config;

/// Serde adaptors that convert between `Vec<OsString>` and JSON sequences.
pub(crate) mod serde_helpers {
    use std::ffi::OsString;

    pub mod os_string_vec {
        use super::OsString;
        use serde::Deserialize;
        use serde::ser::{Error as SerError, SerializeSeq};
        use serde::{Deserializer, Serializer};

        /// Serializes a slice of `OsString` values as a JSON string array,
        /// failing if any element contains non-UTF-8 data that cannot be
        /// represented in JSON.
        pub fn serialize<S>(
            value: &[OsString],
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut seq = serializer.serialize_seq(Some(value.len()))?;
            for arg in value {
                let arg_str = arg.to_str().ok_or_else(|| {
                    SerError::custom(format!(
                        "argument contains non-UTF-8 data: {arg:?}"
                    ))
                })?;
                seq.serialize_element(arg_str)?;
            }
            seq.end()
        }

        /// Deserializes a JSON string array into owned `OsString` values so the
        /// rest of the codebase can operate on platform-native strings.
        pub fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<Vec<OsString>, D::Error>
        where
            D: Deserializer<'de>,
        {
            let strings = Vec::<String>::deserialize(deserializer)?;
            Ok(strings.into_iter().map(OsString::from).collect())
        }
    }
}

/// compile_commands.json entry descriptor
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct CompileCommand {
    pub file: PathBuf,
    pub directory: PathBuf,
    #[serde(with = "crate::serde_helpers::os_string_vec")]
    pub arguments: Vec<OsString>,
}

/// Represents the state of an indexed file within the build tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexedPath {
    /// A uniquely identified parent directory for the file name.
    Unique(PathBuf),
    /// Multiple parent directories were discovered; the entry is ambiguous.
    Ambiguous,
}

impl IndexedPath {
    /// Constructs an `IndexedPath` entry that records a unique parent
    /// directory for the discovered file name.
    pub fn unique(path: PathBuf) -> Self {
        Self::Unique(path)
    }

    /// Marks the entry as conflicting to signal that multiple parents share the
    /// same file name and downstream resolution is ambiguous.
    pub fn mark_conflict(&mut self) {
        *self = Self::Ambiguous;
    }

    /// Returns the associated parent directory if the entry is unique, or
    /// `None` when the index detected ambiguous paths.
    pub fn parent(&self) -> Option<&PathBuf> {
        match self {
            Self::Unique(path) => Some(path),
            Self::Ambiguous => None,
        }
    }
}

/// Core parsing logic - pure functions that can be easily tested
pub mod parser {
    use super::*;
    use std::ffi::OsStr;
    use std::path::Path;

    /// Returns `true` when `value` ends with `suffix`, ignoring ASCII case.
    fn ends_with_ignore_ascii_case(value: &str, suffix: &str) -> bool {
        value
            .get(value.len().saturating_sub(suffix.len())..)
            .map(|tail| tail.eq_ignore_ascii_case(suffix))
            .unwrap_or(false)
    }

    /// Scans a slice of strings and checks whether any entry matches `needle`
    /// while ignoring ASCII case.
    fn slice_contains_ignore_ascii_case(
        slice: &[String],
        needle: &str,
    ) -> bool {
        slice
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(needle))
    }

    /// Compares an `OsStr` against the supplied candidate string ignoring ASCII
    /// case. Non UTF-8 values short-circuit to `false`.
    fn os_str_eq_ignore_ascii_case(value: &OsStr, candidate: &str) -> bool {
        value
            .to_str()
            .map(|val| val.eq_ignore_ascii_case(candidate))
            .unwrap_or(false)
    }

    /// Helper function to check if a line ends with a C/C++ source file extension
    /// (possibly followed by quotes, spaces, or other whitespace)
    pub fn ends_with_cpp_source_file(
        line: &str,
        file_extensions: &[String],
    ) -> bool {
        let line = line.trim_end();
        let line = line.trim_end_matches(['"', '\'']);

        file_extensions
            .iter()
            .any(|ext| ends_with_ignore_ascii_case(line, ext))
    }

    /// Check if a directory should be excluded
    pub fn should_exclude_directory(
        dir_name: &OsStr,
        exclude_directories: &[String],
    ) -> bool {
        exclude_directories
            .iter()
            .any(|exclude| os_str_eq_ignore_ascii_case(dir_name, exclude))
    }

    /// Check if a file extension should be processed
    pub fn should_process_file_extension(
        ext: &OsStr,
        file_extensions: &[String],
    ) -> bool {
        ext.to_str()
            .map(|ext| slice_contains_ignore_ascii_case(file_extensions, ext))
            .unwrap_or(false)
    }

    /// Parse tokens from a compile command line while preserving quoted segments.
    ///
    /// The tokenizer follows Windows command-line quoting conventions:
    /// - Whitespace delimits arguments unless inside double quotes.
    /// - Double quotes are removed while keeping their contents.
    /// - Escaped quotes within quoted segments (e.g. `\"`) are unescaped.
    /// - Empty quoted arguments (e.g. `""`) are preserved as empty strings.
    pub fn tokenize_compile_command(line: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut current = String::new();
        let mut in_quotes = false;
        let mut backslash_count = 0;
        let mut argument_in_progress = false;

        for ch in line.chars() {
            match ch {
                '\\' => {
                    // Defer emitting backslashes until we know whether they
                    // escape a quote or represent literal characters.
                    backslash_count += 1;
                }
                '"' => {
                    if backslash_count > 0 {
                        for _ in 0..(backslash_count / 2) {
                            current.push('\\');
                        }
                    }

                    if backslash_count % 2 == 0 {
                        // An even number of preceding backslashes toggles the
                        // quote state rather than inserting a literal quote.
                        if !argument_in_progress {
                            argument_in_progress = true;
                        }
                        in_quotes = !in_quotes;
                    } else {
                        // Odd backslashes leave an escaped quote in the
                        // argument body.
                        current.push('"');
                        argument_in_progress = true;
                    }

                    backslash_count = 0;
                }
                c if c.is_whitespace() && !in_quotes => {
                    if backslash_count > 0 {
                        // Flush deferred backslashes now that the argument is
                        // clearly continuing.
                        for _ in 0..backslash_count {
                            current.push('\\');
                        }
                        backslash_count = 0;
                        argument_in_progress = true;
                    }

                    if argument_in_progress {
                        tokens.push(std::mem::take(&mut current));
                        argument_in_progress = false;
                    }
                }
                c => {
                    if backslash_count > 0 {
                        for _ in 0..backslash_count {
                            current.push('\\');
                        }
                        backslash_count = 0;
                    }

                    current.push(c);
                    argument_in_progress = true;
                }
            }
        }

        if backslash_count > 0 {
            for _ in 0..backslash_count {
                current.push('\\');
            }
            argument_in_progress = true;
        }

        if argument_in_progress {
            tokens.push(current);
        }

        if !tokens.is_empty() {
            let should_try = {
                let first = &tokens[0];
                first.contains(':') && !is_executable_path(first)
            };

            if should_try {
                let mut merged = tokens[0].clone();
                let mut end_index = None;

                for (idx, part) in tokens.iter().enumerate().skip(1) {
                    // Glue tokens back together until a full executable path
                    // (with a drive prefix) is reconstructed. This accounts
                    // for logs that interleave whitespace inside Windows
                    // paths such as `C:\Program Files`.
                    merged.push(' ');
                    merged.push_str(part);

                    if is_executable_path(&merged) {
                        end_index = Some(idx);
                        break;
                    }
                }

                if let Some(end) = end_index {
                    tokens.splice(0..=end, std::iter::once(merged));
                }
            }
        }

        tokens
    }

    /// Returns `true` when the token looks like a Windows executable path.
    fn is_executable_path(token: &str) -> bool {
        let token = token.trim();
        let lowercase = token.to_ascii_lowercase();
        lowercase.ends_with(".exe")
            || lowercase.ends_with(".com")
            || lowercase.ends_with(".cmd")
            || lowercase.ends_with(".bat")
    }

    /// Extract file name and validate it has an extension
    pub fn extract_and_validate_filename(
        arg_path: &Path,
    ) -> Result<PathBuf, Ms2ccError> {
        let path = arg_path.to_path_buf();
        let file_name = arg_path.file_name().ok_or_else(|| {
            Ms2ccError::MissingFileName { path: path.clone() }
        })?;

        let file_name = PathBuf::from(file_name);

        if file_name.extension().is_none() {
            return Err(Ms2ccError::MissingExtension { path });
        }

        Ok(file_name)
    }
}

/// Compile command creation logic
pub mod compile_commands {
    use super::*;
    use std::ffi::{OsStr, OsString};

    /// Create a CompileCommand from a path and arguments
    pub fn create_compile_command(
        path: PathBuf,
        arguments: Vec<OsString>,
    ) -> Result<CompileCommand, Ms2ccError> {
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| Ms2ccError::MissingParent { path: path.clone() })?
            .to_path_buf();

        let file =
            path.file_name()
                .ok_or_else(|| Ms2ccError::MissingFileName {
                    path: path.clone(),
                })?;
        let file = PathBuf::from(file);

        // We intentionally preserve the raw argument vector so serialization
        // mirrors the exact compiler invocation observed in the log.
        Ok(CompileCommand {
            file,
            directory,
            arguments,
        })
    }

    /// Find /Fo argument in compile arguments
    pub fn find_fo_argument(arguments: &[OsString]) -> Option<&OsString> {
        const ARGUMENT: &str = "/Fo";
        // Matching happens on UTF-8 views since MSBuild emits UTF-16 logs cast
        // to UTF-8 strings in practice. Non UTF-8 arguments are ignored.
        arguments.iter().find(|s| {
            s.to_str()
                .map(|value| value.starts_with(ARGUMENT))
                .unwrap_or(false)
        })
    }

    /// Extract path from /Fo argument
    pub fn extract_fo_path(fo_argument: &OsStr) -> Result<PathBuf, Ms2ccError> {
        const ARGUMENT: &str = "/Fo";
        let argument = fo_argument.to_string_lossy().into_owned();
        let Some(path_segment) = argument.strip_prefix(ARGUMENT) else {
            return Err(Ms2ccError::InvalidFoArgument { argument });
        };
        // The compiler flag may point at either a directory or a file; both are
        // represented via `PathBuf` so later stages can resolve the final
        // source path.
        Ok(PathBuf::from(path_segment))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Unit tests covering the pure parsing helpers that power the CLI.
    mod parser_tests {
        use super::*;
        use std::ffi::OsStr;
        use std::path::Path;

        // Confirms the helper spots valid C/C++ extensions in realistic input.
        #[test]
        fn test_ends_with_cpp_source_file() {
            let extensions =
                vec!["cpp".to_string(), "c".to_string(), "h".to_string()];

            assert!(parser::ends_with_cpp_source_file("file.cpp", &extensions));
            assert!(parser::ends_with_cpp_source_file("file.CPP", &extensions));
            assert!(parser::ends_with_cpp_source_file(
                "file.cpp\"  ",
                &extensions
            ));
            assert!(parser::ends_with_cpp_source_file("file.h'", &extensions));
            assert!(!parser::ends_with_cpp_source_file("file", &extensions));
            assert!(!parser::ends_with_cpp_source_file("", &extensions));
        }

        // Ensures directory names are excluded using case-insensitive matching.
        #[test]
        fn test_should_exclude_directory() {
            let excludes = vec![".git".to_string(), "target".to_string()];

            assert!(parser::should_exclude_directory(
                OsStr::new(".git"),
                &excludes
            ));
            assert!(parser::should_exclude_directory(
                OsStr::new(".GIT"),
                &excludes
            ));
            assert!(parser::should_exclude_directory(
                OsStr::new("target"),
                &excludes
            ));
            assert!(!parser::should_exclude_directory(
                OsStr::new("src"),
                &excludes
            ));
        }

        // Guards against false negatives when the exclusion list stores strings as `OsString`.
        #[test]
        fn test_should_exclude_directory_with_osstring() {
            let excludes = vec![".git".to_string()];
            let dir = std::ffi::OsString::from(".Git");
            assert!(parser::should_exclude_directory(
                dir.as_os_str(),
                &excludes
            ));
        }

        // Verifies extension filtering handles both upper and lower case values.
        #[test]
        fn test_should_process_file_extension() {
            let extensions = vec!["cpp".to_string(), "h".to_string()];

            assert!(parser::should_process_file_extension(
                OsStr::new("cpp"),
                &extensions
            ));
            assert!(parser::should_process_file_extension(
                OsStr::new("CPP"),
                &extensions
            ));
            assert!(parser::should_process_file_extension(
                OsStr::new("h"),
                &extensions
            ));
            assert!(!parser::should_process_file_extension(
                OsStr::new("txt"),
                &extensions
            ));
        }

        // Tokenizes a straightforward single-line command.
        #[test]
        fn test_tokenize_compile_command() {
            let line = "cl.exe /c /Zi file.cpp";
            let tokens = parser::tokenize_compile_command(line);
            assert_eq!(tokens, vec!["cl.exe", "/c", "/Zi", "file.cpp"]);
        }

        // Handles quoted executable paths and include directories with spaces.
        #[test]
        fn test_tokenize_compile_command_with_quotes() {
            let line =
                r#""C:\Program Files\cl.exe" /c /I"C:\Some Path" main.cpp"#;
            let tokens = parser::tokenize_compile_command(line);
            assert_eq!(
                tokens,
                vec![
                    "C:\\Program Files\\cl.exe",
                    "/c",
                    "/IC:\\Some Path",
                    "main.cpp",
                ]
            );
        }

        // Keeps empty quoted arguments instead of dropping them.
        #[test]
        fn test_tokenize_compile_command_with_empty_argument() {
            let line = r#"cl.exe "" "C:\path with spaces\file.cpp""#;
            let tokens = parser::tokenize_compile_command(line);
            assert_eq!(
                tokens,
                vec!["cl.exe", "", "C:\\path with spaces\\file.cpp",]
            );
        }

        // Properly unescapes embedded quotes within arguments.
        #[test]
        fn test_tokenize_compile_command_with_escaped_quote() {
            let line = r#"cl.exe "/D\"VALUE\"" main.cpp"#;
            let tokens = parser::tokenize_compile_command(line);
            assert_eq!(tokens, vec!["cl.exe", "/D\"VALUE\"", "main.cpp"]);
        }

        // Ensures trailing backslashes inside include paths are preserved.
        #[test]
        fn test_tokenize_compile_command_with_trailing_backslash() {
            let line = r#"cl.exe /I"C:\include\\" main.cpp"#;
            let tokens = parser::tokenize_compile_command(line);
            assert_eq!(tokens, vec!["cl.exe", "/IC:\\include\\", "main.cpp"]);
        }

        // Merges spaced tokens back into an unquoted executable path.
        #[test]
        fn test_tokenize_compile_command_unquoted_executable_path() {
            let line = r#"C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Tools\MSVC\14.44.35207\bin\HostX64\x64\CL.exe /c main.cpp"#;
            let tokens = parser::tokenize_compile_command(line);
            assert_eq!(
                tokens,
                vec![
                    "C:\\Program Files\\Microsoft Visual Studio\\2022\\Enterprise\\VC\\Tools\\MSVC\\14.44.35207\\bin\\HostX64\\x64\\CL.exe",
                    "/c",
                    "main.cpp",
                ]
            );
        }

        // Accepts well-formed paths, returning their file component.
        #[test]
        fn test_extract_and_validate_filename_success() {
            let result =
                parser::extract_and_validate_filename(Path::new("file.cpp"));
            assert!(matches!(
                result,
                Ok(ref value) if value.as_path() == Path::new("file.cpp")
            ));

            let result = parser::extract_and_validate_filename(Path::new(
                "path/to/file.cpp",
            ));
            assert!(matches!(
                result,
                Ok(ref value) if value.as_path() == Path::new("file.cpp")
            ));
        }

        // Rejects paths missing file extensions.
        #[test]
        fn test_extract_and_validate_filename_missing_extension() {
            let result =
                parser::extract_and_validate_filename(Path::new("file"));
            assert!(matches!(
                result,
                Err(Ms2ccError::MissingExtension { ref path })
                    if path.as_path() == Path::new("file")
            ));
        }

        // Rejects paths without a terminal file name.
        #[test]
        fn test_extract_and_validate_filename_missing_file_name() {
            let result = parser::extract_and_validate_filename(Path::new(""));
            assert!(matches!(result, Err(Ms2ccError::MissingFileName { .. })));
        }

        // Confirms path casing is preserved when returning the validated name.
        #[test]
        fn test_extract_and_validate_filename_preserves_case() {
            let path = Path::new("SubDir/FiLe.CpP");
            let result = parser::extract_and_validate_filename(path).unwrap();
            assert_eq!(result, PathBuf::from("FiLe.CpP"));
        }

        #[cfg(unix)]
        // Ensures non-UTF file names are passed through unchanged on Unix.
        #[test]
        fn test_extract_and_validate_filename_non_utf() {
            use std::os::unix::ffi::OsStringExt;

            let bytes = vec![0xFF, b'.', b'c', b'p', b'p'];
            let os_string = std::ffi::OsString::from_vec(bytes.clone());
            let path = PathBuf::from(os_string.clone());

            let result =
                parser::extract_and_validate_filename(path.as_path()).unwrap();
            use std::os::unix::ffi::OsStrExt;
            assert_eq!(result.as_os_str().as_bytes(), &bytes);
        }
    }

    // Unit tests focused on compile command construction helpers.
    mod compile_commands_tests {
        use super::*;
        use std::ffi::{OsStr, OsString};

        // Builds a compile command from a fully-qualified source path.
        #[test]
        fn test_create_compile_command() {
            let path = PathBuf::from("C:/projects/src/file.cpp");
            let args = vec![
                OsString::from("cl.exe"),
                OsString::from("/c"),
                OsString::from("file.cpp"),
            ];

            let result =
                compile_commands::create_compile_command(path, args.clone());
            assert!(result.is_ok());

            let cmd = result.unwrap();
            assert_eq!(cmd.file, PathBuf::from("file.cpp"));
            assert_eq!(cmd.directory, PathBuf::from("C:/projects/src"));
            assert_eq!(cmd.arguments, args);
        }

        // Locates the `/Fo` argument when present among compiler flags.
        #[test]
        fn test_find_fo_argument() {
            let args = vec![
                OsString::from("cl.exe"),
                OsString::from("/FoDebug/"),
                OsString::from("file.cpp"),
            ];

            let fo_arg = compile_commands::find_fo_argument(&args);
            assert_eq!(
                fo_arg.map(|value| value.as_os_str()),
                Some(OsStr::new("/FoDebug/"))
            );

            let args_no_fo =
                vec![OsString::from("cl.exe"), OsString::from("file.cpp")];
            assert!(compile_commands::find_fo_argument(&args_no_fo).is_none());
        }

        // Parses `/Fo` arguments into usable filesystem paths.
        #[test]
        fn test_extract_fo_path() {
            let result =
                compile_commands::extract_fo_path(OsStr::new("/FoDebug/obj/"));
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), PathBuf::from("Debug/obj/"));

            let invalid =
                compile_commands::extract_fo_path(OsStr::new("invalid"));
            assert!(matches!(
                invalid,
                Err(Ms2ccError::InvalidFoArgument { argument })
                    if argument == "invalid"
            ));
        }

        // Emits a structured error when the source path lacks a parent directory.
        #[test]
        fn test_create_compile_command_missing_parent() {
            let path = PathBuf::from("file.cpp");
            let result = compile_commands::create_compile_command(
                path.clone(),
                vec!["cl.exe".into()],
            );
            assert!(matches!(
                result,
                Err(Ms2ccError::MissingParent { path: p }) if p == path
            ));
        }

        // Double-checks compile commands survive a JSON serialization cycle.
        #[test]
        fn test_compile_command_serialization_roundtrip() {
            let command = CompileCommand {
                file: PathBuf::from("file.cpp"),
                directory: PathBuf::from("/tmp/project"),
                arguments: vec![
                    OsString::from("cl.exe"),
                    OsString::from("/c"),
                    OsString::from("file.cpp"),
                ],
            };

            let json = serde_json::to_string(&command).expect("serialize");
            let rebuilt: CompileCommand =
                serde_json::from_str(&json).expect("deserialize");

            assert_eq!(rebuilt, command);
        }
    }
}
