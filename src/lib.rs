// lib.rs - Core library functions for ms2cc

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// compile_commands.json entry descriptor
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct CompileCommand {
    pub file: PathBuf,
    pub directory: PathBuf,
    pub arguments: Vec<String>,
}

/// Configuration for the ms2cc tool
#[derive(Debug, Clone)]
pub struct Config {
    pub exclude_directories: Vec<String>,
    pub file_extensions: Vec<String>,
    pub compiler_executable: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            exclude_directories: vec![".git".to_string()],
            file_extensions: vec![
                "c".to_string(),
                "cc".to_string(),
                "cpp".to_string(),
                "cxx".to_string(),
                "c++".to_string(),
                "h".to_string(),
                "hh".to_string(),
                "hpp".to_string(),
                "hxx".to_string(),
                "h++".to_string(),
                "inl".to_string(),
            ],
            compiler_executable: "cl.exe".to_string(),
        }
    }
}

/// Core parsing logic - pure functions that can be easily tested
pub mod parser {
    use super::*;

    /// Helper function to check if a line ends with a C/C++ source file extension
    /// (possibly followed by quotes, spaces, or other whitespace)
    pub fn ends_with_cpp_source_file(
        line: &str,
        file_extensions: &[String],
    ) -> bool {
        let line = line.trim_end(); // Remove trailing whitespace
        let line = line.trim_end_matches(['"', '\'']); // Remove trailing quotes

        // Check for C/C++ source file extensions
        file_extensions
            .iter()
            .any(|ext| line.to_lowercase().ends_with(&ext.to_lowercase()))
    }

    /// Check if a directory should be excluded
    pub fn should_exclude_directory(
        dir_name: &str,
        exclude_directories: &[String],
    ) -> bool {
        let dir_name_lower = dir_name.to_lowercase();
        exclude_directories
            .iter()
            .any(|exclude| exclude.to_lowercase() == dir_name_lower)
    }

    /// Check if a file extension should be processed
    pub fn should_process_file_extension(
        ext: &str,
        file_extensions: &[String],
    ) -> bool {
        let ext_lower = ext.to_lowercase();
        file_extensions
            .iter()
            .any(|allowed_ext| allowed_ext.to_lowercase() == ext_lower)
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
                    backslash_count += 1;
                }
                '"' => {
                    if backslash_count > 0 {
                        for _ in 0..(backslash_count / 2) {
                            current.push('\\');
                        }
                    }

                    if backslash_count % 2 == 0 {
                        if !argument_in_progress {
                            argument_in_progress = true;
                        }
                        in_quotes = !in_quotes;
                    } else {
                        current.push('"');
                        argument_in_progress = true;
                    }

                    backslash_count = 0;
                }
                c if c.is_whitespace() && !in_quotes => {
                    if backslash_count > 0 {
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
        arg_path: &str,
    ) -> Result<PathBuf, String> {
        let arg_path_buf = PathBuf::from(arg_path.to_lowercase());

        let file_name = arg_path_buf.file_name().ok_or_else(|| {
            format!("Missing file_name component in {arg_path:?}")
        })?;

        let file_name = PathBuf::from(file_name);

        if file_name.extension().is_none() {
            return Err(format!(
                "File name component missing extension {arg_path:?}"
            ));
        }

        Ok(file_name)
    }
}

/// Compile command creation logic
pub mod compile_commands {
    use super::*;

    /// Create a CompileCommand from a path and arguments
    pub fn create_compile_command(
        path: PathBuf,
        arguments: Vec<String>,
    ) -> Result<CompileCommand, String> {
        let directory = path
            .parent()
            .ok_or_else(|| format!("Missing parent component in {:?}", path))?
            .to_path_buf();

        let file = path.file_name().ok_or_else(|| {
            format!("Missing file_name component in {:?}", path)
        })?;
        let file = PathBuf::from(file);

        Ok(CompileCommand {
            file,
            directory,
            arguments,
        })
    }

    /// Find /Fo argument in compile arguments
    pub fn find_fo_argument(arguments: &[String]) -> Option<&String> {
        const ARGUMENT: &str = "/Fo";
        arguments.iter().find(|s| s.starts_with(ARGUMENT))
    }

    /// Extract path from /Fo argument
    pub fn extract_fo_path(fo_argument: &str) -> Result<PathBuf, String> {
        const ARGUMENT: &str = "/Fo";
        let path_str = fo_argument
            .strip_prefix(ARGUMENT)
            .ok_or("Invalid /Fo argument format")?;
        Ok(PathBuf::from(path_str.to_lowercase()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod parser_tests {
        use super::*;

        #[test]
        fn test_ends_with_cpp_source_file() {
            let extensions =
                vec!["cpp".to_string(), "c".to_string(), "h".to_string()];

            assert!(parser::ends_with_cpp_source_file("file.cpp", &extensions));
            assert!(parser::ends_with_cpp_source_file(
                "file.cpp\"",
                &extensions
            ));
            assert!(parser::ends_with_cpp_source_file(
                "  file.cpp  ",
                &extensions
            ));
            assert!(parser::ends_with_cpp_source_file("FILE.CPP", &extensions));
            assert!(!parser::ends_with_cpp_source_file(
                "file.txt",
                &extensions
            ));
            assert!(!parser::ends_with_cpp_source_file("file", &extensions));
        }

        #[test]
        fn test_should_exclude_directory() {
            let excludes = vec![".git".to_string(), "target".to_string()];

            assert!(parser::should_exclude_directory(".git", &excludes));
            assert!(parser::should_exclude_directory(".GIT", &excludes));
            assert!(parser::should_exclude_directory("target", &excludes));
            assert!(!parser::should_exclude_directory("src", &excludes));
        }

        #[test]
        fn test_should_process_file_extension() {
            let extensions = vec!["cpp".to_string(), "h".to_string()];

            assert!(parser::should_process_file_extension("cpp", &extensions));
            assert!(parser::should_process_file_extension("CPP", &extensions));
            assert!(parser::should_process_file_extension("h", &extensions));
            assert!(!parser::should_process_file_extension("txt", &extensions));
        }

        #[test]
        fn test_tokenize_compile_command() {
            let line = "cl.exe /c /Zi file.cpp";
            let tokens = parser::tokenize_compile_command(line);
            assert_eq!(tokens, vec!["cl.exe", "/c", "/Zi", "file.cpp"]);
        }

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

        #[test]
        fn test_tokenize_compile_command_with_empty_argument() {
            let line = r#"cl.exe "" "C:\path with spaces\file.cpp""#;
            let tokens = parser::tokenize_compile_command(line);
            assert_eq!(
                tokens,
                vec!["cl.exe", "", "C:\\path with spaces\\file.cpp",]
            );
        }

        #[test]
        fn test_tokenize_compile_command_with_escaped_quote() {
            let line = r#"cl.exe "/D\"VALUE\"" main.cpp"#;
            let tokens = parser::tokenize_compile_command(line);
            assert_eq!(tokens, vec!["cl.exe", "/D\"VALUE\"", "main.cpp"]);
        }

        #[test]
        fn test_tokenize_compile_command_with_trailing_backslash() {
            let line = r#"cl.exe /I"C:\include\\" main.cpp"#;
            let tokens = parser::tokenize_compile_command(line);
            assert_eq!(tokens, vec!["cl.exe", "/IC:\\include\\", "main.cpp"]);
        }

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

        #[test]
        fn test_extract_and_validate_filename() {
            assert!(parser::extract_and_validate_filename("file.cpp").is_ok());
            assert!(
                parser::extract_and_validate_filename("path/file.cpp").is_ok()
            );
            assert!(parser::extract_and_validate_filename("file").is_err());
            assert!(parser::extract_and_validate_filename("").is_err());
        }
    }

    mod compile_commands_tests {
        use super::*;

        #[test]
        fn test_create_compile_command() {
            let path = PathBuf::from("C:/projects/src/file.cpp");
            let args = vec![
                "cl.exe".to_string(),
                "/c".to_string(),
                "file.cpp".to_string(),
            ];

            let result =
                compile_commands::create_compile_command(path, args.clone());
            assert!(result.is_ok());

            let cmd = result.unwrap();
            assert_eq!(cmd.file, PathBuf::from("file.cpp"));
            assert_eq!(cmd.directory, PathBuf::from("C:/projects/src"));
            assert_eq!(cmd.arguments, args);
        }

        #[test]
        fn test_find_fo_argument() {
            let args = vec![
                "cl.exe".to_string(),
                "/FoDebug/".to_string(),
                "file.cpp".to_string(),
            ];

            let fo_arg = compile_commands::find_fo_argument(&args);
            assert_eq!(fo_arg, Some(&"/FoDebug/".to_string()));

            let args_no_fo = vec!["cl.exe".to_string(), "file.cpp".to_string()];
            assert!(compile_commands::find_fo_argument(&args_no_fo).is_none());
        }

        #[test]
        fn test_extract_fo_path() {
            let result = compile_commands::extract_fo_path("/FoDebug/obj/");
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), PathBuf::from("debug/obj/"));

            let invalid = compile_commands::extract_fo_path("invalid");
            assert!(invalid.is_err());
        }
    }
}
