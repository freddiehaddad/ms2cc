// tests/integration_tests.rs - Integration tests for ms2cc

use ms2cc::{Config, parser, compile_commands};
use std::path::PathBuf;
use tempfile::TempDir;
use std::fs::File;
use std::io::Write;

#[test]
fn test_end_to_end_parsing_workflow() {
    // Test the complete workflow of parsing a compile command
    let line = r#"cl.exe /c /Zi /nologo /W3 "C:\projects\src\main.cpp""#;
    
    // Step 1: Clean the line
    let cleaned = parser::cleanup_line(line);
    assert!(!cleaned.contains('"'));
    
    // Step 2: Tokenize
    let tokens = parser::tokenize_compile_command(&cleaned);
    assert!(tokens.len() >= 2);
    assert_eq!(tokens[0], "cl.exe");
    assert!(tokens.last().unwrap().ends_with("main.cpp"));
    
    // Step 3: Extract filename
    let filename_result = parser::extract_and_validate_filename(tokens.last().unwrap());
    assert!(filename_result.is_ok());
}

#[test]
fn test_file_extension_filtering() {
    let config = Config::default();
    
    // Test valid extensions
    assert!(parser::should_process_file_extension("cpp", &config.file_extensions));
    assert!(parser::should_process_file_extension("h", &config.file_extensions));
    assert!(parser::should_process_file_extension("c", &config.file_extensions));
    
    // Test invalid extensions
    assert!(!parser::should_process_file_extension("txt", &config.file_extensions));
    assert!(!parser::should_process_file_extension("exe", &config.file_extensions));
    assert!(!parser::should_process_file_extension("", &config.file_extensions));
}

#[test]
fn test_directory_exclusion() {
    let config = Config::default();
    
    // Test default exclusions
    assert!(parser::should_exclude_directory(".git", &config.exclude_directories));
    assert!(parser::should_exclude_directory(".GIT", &config.exclude_directories)); // case insensitive
    
    // Test non-excluded directories
    assert!(!parser::should_exclude_directory("src", &config.exclude_directories));
    assert!(!parser::should_exclude_directory("include", &config.exclude_directories));
}

#[test]
fn test_compile_command_creation_with_fo_argument() {
    let arguments = vec![
        "cl.exe".to_string(),
        "/c".to_string(),
        "/FoDebug\\obj\\".to_string(),
        "main.cpp".to_string(),
    ];
    
    // Test finding /Fo argument
    let fo_arg = compile_commands::find_fo_argument(&arguments);
    assert!(fo_arg.is_some());
    
    // Test extracting path
    let fo_path_result = compile_commands::extract_fo_path(fo_arg.unwrap());
    assert!(fo_path_result.is_ok());
}

#[test]
fn test_multiline_compile_command_detection() {
    let config = Config::default();
    
    // Test complete single-line command
    let complete_line = "cl.exe /c /Zi main.cpp";
    assert!(parser::ends_with_cpp_source_file(complete_line, &config.file_extensions));
    
    // Test incomplete command (no source file)
    let incomplete_line = "cl.exe /c /Zi /nologo";
    assert!(!parser::ends_with_cpp_source_file(incomplete_line, &config.file_extensions));
    
    // Test with quotes and whitespace
    let quoted_line = r#"cl.exe /c "main.cpp"   "#;
    assert!(parser::ends_with_cpp_source_file(quoted_line, &config.file_extensions));
}

#[cfg(test)]
mod temp_file_tests {
    use super::*;
    
    #[test]
    fn test_with_temporary_files() {
        let temp_dir = TempDir::new().unwrap();
        
        // Create test source file
        let cpp_file = temp_dir.path().join("test.cpp");
        let mut file = File::create(&cpp_file).unwrap();
        writeln!(file, "int main() {{ return 0; }}").unwrap();
        
        // Test creating compile command with real path
        let arguments = vec!["cl.exe".to_string(), "/c".to_string(), "test.cpp".to_string()];
        let result = compile_commands::create_compile_command(cpp_file, arguments.clone());
        
        assert!(result.is_ok());
        let cmd = result.unwrap();
        assert_eq!(cmd.file, PathBuf::from("test.cpp"));
        assert_eq!(cmd.directory, temp_dir.path());
        assert_eq!(cmd.arguments, arguments);
    }
}
