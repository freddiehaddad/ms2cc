//! Integration-like tests that use temporary directories to validate filesystem
//! traversal helpers.

use ms2cc::{Config, parser};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::path::Path;
use tempfile::TempDir;

/// Verifies extension filtering accepts only C/C++ files inside a real
/// directory.
#[test]
fn test_file_filtering_in_real_directory() {
    let temp_dir = TempDir::new().unwrap();
    let config = Config::default();

    // Create test files with different extensions
    let files_to_create = [
        "main.cpp",
        "header.h",
        "source.c",
        "readme.txt",
        "makefile",
        "test.hpp",
        "script.py",
    ];

    for filename in &files_to_create {
        let file_path = temp_dir.path().join(filename);
        File::create(&file_path).unwrap();
    }

    // Test which files should be processed
    let entries = fs::read_dir(temp_dir.path()).unwrap();
    let mut processed_files = Vec::new();

    for entry in entries {
        let entry = entry.unwrap();
        let path = entry.path();

        if path.is_file()
            && let Some(ext) = path.extension()
            && parser::should_process_file_extension(
                ext,
                &config.file_extensions,
            )
        {
            processed_files
                .push(path.file_name().unwrap().to_string_lossy().to_string());
        }
    }

    // Should process C/C++ files but not others
    assert!(processed_files.contains(&"main.cpp".to_string()));
    assert!(processed_files.contains(&"header.h".to_string()));
    assert!(processed_files.contains(&"source.c".to_string()));
    assert!(processed_files.contains(&"test.hpp".to_string()));
    assert!(!processed_files.contains(&"readme.txt".to_string()));
    assert!(!processed_files.contains(&"makefile".to_string()));
    assert!(!processed_files.contains(&"script.py".to_string()));
}

/// Confirms default configuration excludes only the expected directories.
#[test]
fn test_directory_exclusion_in_real_filesystem() {
    let temp_dir = TempDir::new().unwrap();
    let config = Config::default();

    // Create test directories
    let dirs_to_create =
        [".git", "src", "include", "target", "build", ".vscode"];

    for dirname in &dirs_to_create {
        let dir_path = temp_dir.path().join(dirname);
        fs::create_dir(&dir_path).unwrap();

        // Add a file to each directory to make them non-empty
        let file_path = dir_path.join("dummy.txt");
        File::create(&file_path).unwrap();
    }

    // Test which directories should be excluded
    let entries = fs::read_dir(temp_dir.path()).unwrap();
    let mut processed_dirs = Vec::new();

    for entry in entries {
        let entry = entry.unwrap();
        let path = entry.path();

        if path.is_dir()
            && let Some(dir_name) = path.file_name()
            && !parser::should_exclude_directory(
                dir_name,
                &config.exclude_directories,
            )
        {
            processed_dirs.push(dir_name.to_string_lossy().into_owned());
        }
    }

    // Should exclude .git but not others (with default config)
    assert!(!processed_dirs.contains(&".git".to_string()));
    assert!(processed_dirs.contains(&"src".to_string()));
    assert!(processed_dirs.contains(&"include".to_string()));
    assert!(processed_dirs.contains(&"target".to_string()));
    assert!(processed_dirs.contains(&"build".to_string()));
    assert!(processed_dirs.contains(&".vscode".to_string()));
}

/// Checks that nested directory structures remain traversable and extensions
/// are still recognized.
#[test]
fn test_nested_directory_structure() {
    let temp_dir = TempDir::new().unwrap();
    let config = Config::default();

    // Create nested directory structure
    let nested_path = temp_dir.path().join("src").join("components");
    fs::create_dir_all(&nested_path).unwrap();

    // Create files in nested directories
    let main_cpp = temp_dir.path().join("src").join("main.cpp");
    let component_h = nested_path.join("component.h");

    File::create(&main_cpp).unwrap();
    File::create(&component_h).unwrap();

    // Verify the files exist and would be processed
    assert!(main_cpp.exists());
    assert!(component_h.exists());

    // Test file extension processing
    assert!(parser::should_process_file_extension(
        OsStr::new("cpp"),
        &config.file_extensions
    ));
    assert!(parser::should_process_file_extension(
        OsStr::new("h"),
        &config.file_extensions
    ));
}

/// Ensures path normalization preserves case while still matching extensions.
#[test]
fn test_path_normalization() {
    let config = Config::default();
    let test_cases = ["MAIN.CPP", "Header.H", "Source.CXX", "Test.hpp"];

    for input in &test_cases {
        let path = Path::new(input);
        let file_name = parser::extract_and_validate_filename(path).unwrap();

        assert_eq!(file_name.to_str().unwrap(), *input);

        if let Some(ext) = file_name.extension() {
            assert!(parser::should_process_file_extension(
                ext,
                &config.file_extensions
            ));
        }
    }
}

/// Covers edge cases for filenames lacking extensions or using uncommon
/// casing.
#[test]
fn test_edge_cases_with_file_paths() {
    let config = Config::default();

    // Test files without extensions
    assert!(!parser::should_process_file_extension(
        OsStr::new(""),
        &config.file_extensions
    ));

    // Test with dots in filename but valid extension
    assert!(parser::should_process_file_extension(
        OsStr::new("cpp"),
        &config.file_extensions
    ));

    // Test case insensitive extension matching
    assert!(parser::should_process_file_extension(
        OsStr::new("CPP"),
        &config.file_extensions
    ));
    assert!(parser::should_process_file_extension(
        OsStr::new("Hpp"),
        &config.file_extensions
    ));
    assert!(parser::should_process_file_extension(
        OsStr::new("C"),
        &config.file_extensions
    ));
}
