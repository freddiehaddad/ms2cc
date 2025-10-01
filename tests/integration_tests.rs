//! High-level integration tests exercising parser and compile command helpers
//! together.

use ms2cc::{Config, compile_commands, parser};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use tempfile::TempDir;

/// Walks through tokenizing a realistic compile command and extracting the
/// trailing filename.
#[test]
fn test_end_to_end_parsing_workflow() {
    // Test the complete workflow of parsing a compile command
    let line = r#"cl.exe /c /Zi /nologo /W3 "C:\projects\src\main.cpp""#;

    // Tokenize the raw line directly
    let tokens = parser::tokenize_compile_command(line);
    assert!(tokens.len() >= 2);
    assert_eq!(tokens[0], "cl.exe");
    assert!(tokens.last().unwrap().ends_with("main.cpp"));

    // Extract filename
    let filename_result = parser::extract_and_validate_filename(Path::new(
        tokens.last().unwrap(),
    ));
    assert!(filename_result.is_ok());
}

/// Validates inclusion/exclusion of common file extensions.
#[test]
fn test_file_extension_filtering() {
    let config = Config::default();

    // Test valid extensions
    assert!(parser::should_process_file_extension(
        OsStr::new("cpp"),
        &config.file_extensions
    ));
    assert!(parser::should_process_file_extension(
        OsStr::new("h"),
        &config.file_extensions
    ));
    assert!(parser::should_process_file_extension(
        OsStr::new("c"),
        &config.file_extensions
    ));

    // Test invalid extensions
    assert!(!parser::should_process_file_extension(
        OsStr::new("txt"),
        &config.file_extensions
    ));
    assert!(!parser::should_process_file_extension(
        OsStr::new("exe"),
        &config.file_extensions
    ));
    assert!(!parser::should_process_file_extension(
        OsStr::new(""),
        &config.file_extensions
    ));
}

/// Ensures directory filtering honors default exclusions.
#[test]
fn test_directory_exclusion() {
    let config = Config::default();

    // Test default exclusions
    assert!(parser::should_exclude_directory(
        OsStr::new(".git"),
        &config.exclude_directories
    ));
    assert!(parser::should_exclude_directory(
        OsStr::new(".GIT"),
        &config.exclude_directories
    )); // case insensitive

    // Test non-excluded directories
    assert!(!parser::should_exclude_directory(
        OsStr::new("src"),
        &config.exclude_directories
    ));
    assert!(!parser::should_exclude_directory(
        OsStr::new("include"),
        &config.exclude_directories
    ));
}

/// Confirms `/Fo` flags can be located and parsed successfully.
#[test]
fn test_compile_command_creation_with_fo_argument() {
    let arguments = vec![
        OsString::from("cl.exe"),
        OsString::from("/c"),
        OsString::from("/FoDebug\\obj\\"),
        OsString::from("main.cpp"),
    ];

    // Test finding /Fo argument
    let fo_arg = compile_commands::find_fo_argument(&arguments);
    assert!(fo_arg.is_some());

    // Test extracting path
    let fo_path_result =
        compile_commands::extract_fo_path(fo_arg.unwrap().as_os_str());
    assert!(fo_path_result.is_ok());
}

/// Detects whether compile commands span single or multiple lines in logs.
#[test]
fn test_multiline_compile_command_detection() {
    let config = Config::default();

    // Test complete single-line command
    let complete_line = "cl.exe /c /Zi main.cpp";
    assert!(parser::ends_with_cpp_source_file(
        complete_line,
        &config.file_extensions
    ));

    // Test incomplete command (no source file)
    let incomplete_line = "cl.exe /c /Zi /nologo";
    assert!(!parser::ends_with_cpp_source_file(
        incomplete_line,
        &config.file_extensions
    ));

    // Test with quotes and whitespace
    let quoted_line = r#"cl.exe /c "main.cpp"   "#;
    assert!(parser::ends_with_cpp_source_file(
        quoted_line,
        &config.file_extensions
    ));
}

#[cfg(test)]
mod temp_file_tests {
    use super::*;

    /// Creates real temporary files to validate compile command generation.
    #[test]
    fn test_with_temporary_files() {
        let temp_dir = TempDir::new().unwrap();

        // Create test source file
        let cpp_file = temp_dir.path().join("test.cpp");
        let mut file = File::create(&cpp_file).unwrap();
        writeln!(file, "int main() {{ return 0; }}").unwrap();

        // Test creating compile command with real path
        let arguments = vec![
            OsString::from("cl.exe"),
            OsString::from("/c"),
            OsString::from("test.cpp"),
        ];
        let result = compile_commands::create_compile_command(
            cpp_file,
            arguments.clone(),
        );

        assert!(result.is_ok());
        let cmd = result.unwrap();
        assert_eq!(cmd.file, PathBuf::from("test.cpp"));
        assert_eq!(cmd.directory, temp_dir.path());
        assert_eq!(cmd.arguments, arguments);
    }
}
