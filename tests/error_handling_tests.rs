//! Regression and edge-case tests covering the library’s structured error
//! behavior and resilience against malformed inputs.

use ms2cc::{Config, Ms2ccError, compile_commands, parser};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Verifies the parser rejects malformed compile command inputs without
/// panicking and strips quotes from tokens.
#[test]
fn test_malformed_input_handling() {
    let _config = Config::default();

    // Test malformed compile commands
    let malformed_cases = [
        "",                                 // Empty string
        "not a compile command",            // No compiler
        "cl.exe /c",                        // No source file
        "cl.exe /c file_without_extension", // No extension
        "cl.exe /c .cpp",                   // Extension only
    ];

    for case in malformed_cases {
        // Test tokenization
        let tokens = parser::tokenize_compile_command(case);

        // Test filename extraction if we have tokens
        if !tokens.is_empty() {
            let last_token = tokens.last().unwrap();

            // Should handle gracefully
            let result =
                parser::extract_and_validate_filename(Path::new(last_token));
            assert!(result.is_err());
        }

        // Tokens should not retain quotes
        for token in tokens {
            assert!(
                !token.contains('"'),
                "Token should not retain quotes: {token:?}"
            );
        }
    }
}

/// Ensures parser helpers surface structured errors for missing file
/// components.
#[test]
fn test_structured_errors_from_parser() {
    let missing_extension =
        parser::extract_and_validate_filename(Path::new("file"));
    assert!(matches!(
        missing_extension,
        Err(Ms2ccError::MissingExtension { .. })
    ));

    let missing_file_name =
        parser::extract_and_validate_filename(Path::new("/"));
    assert!(matches!(
        missing_file_name,
        Err(Ms2ccError::MissingFileName { .. })
    ));
}

/// Confirms compile command helpers map validation failures to the
/// appropriate `Ms2ccError` variants.
#[test]
fn test_structured_errors_from_compile_commands() {
    let err = compile_commands::create_compile_command(
        PathBuf::from("file.cpp"),
        Vec::new(),
    )
    .unwrap_err();
    assert!(matches!(err, Ms2ccError::MissingParent { .. }));

    let fo_err =
        compile_commands::extract_fo_path(OsStr::new("/FXbad")).unwrap_err();
    assert!(matches!(
        fo_err,
        Ms2ccError::InvalidFoArgument { argument } if argument == "/FXbad"
    ));
}

/// Exercises boundary conditions such as long inputs, excessive quoting, and
/// unusual directory or extension names.
#[test]
fn test_boundary_conditions() {
    let config = Config::default();

    // Test with very long strings
    let long_string = "a".repeat(10000);
    let tokens = parser::tokenize_compile_command(&long_string);
    assert_eq!(tokens, vec![long_string.clone()]);

    // Test with many quotes
    let many_quotes = "\"".repeat(1000);
    let tokens = parser::tokenize_compile_command(&many_quotes);
    assert_eq!(tokens, vec![String::new()]);

    // Test with mixed content
    let mixed = format!(
        "{}cl.exe{} /c{} main.cpp{}",
        "\"".repeat(100),
        "\"".repeat(100),
        "\"".repeat(100),
        "\"".repeat(100)
    );
    let tokens = parser::tokenize_compile_command(&mixed);
    assert!(tokens.iter().any(|t| t.contains("cl.exe")));
    assert!(tokens.iter().any(|t| t.contains("main.cpp")));
    assert!(tokens.iter().all(|t| !t.contains('"')));

    // Test directory name edge cases
    let edge_case_dirs = [
        "",
        ".",
        "..",
        "very_long_directory_name_that_might_cause_issues",
        "dir-with-dashes",
        "dir.with.dots",
    ];

    for dir in edge_case_dirs {
        let result = parser::should_exclude_directory(
            OsStr::new(dir),
            &config.exclude_directories,
        );
        assert!(!result);
    }

    // Test file extension edge cases
    let edge_case_exts = [
        ("", false),
        ("c", true),
        ("C", true),
        ("cPP", true),
        ("h++", true),
        ("very_long_extension", false),
        ("1", false),
        ("123", false),
    ];

    for (ext, expected) in edge_case_exts {
        let actual = parser::should_process_file_extension(
            OsStr::new(ext),
            &config.file_extensions,
        );
        // Should not panic and return a boolean
        assert_eq!(actual, expected);
    }
}

/// Validates Unicode and special-character handling during tokenization.
#[test]
fn test_unicode_and_special_characters() {
    // Test with Unicode characters
    let unicode_cases = [
        "cl.exe /c файл.cpp",       // Cyrillic
        "cl.exe /c 测试.cpp",       // Chinese
        "cl.exe /c тест.cpp",       // More Cyrillic
        "cl.exe /c プログラム.cpp", // Japanese
        "cl.exe /c 🚀test.cpp",     // Emoji
    ];

    for case in &unicode_cases {
        let tokens = parser::tokenize_compile_command(case);
        assert!(tokens.len() >= 3, "Should tokenize Unicode strings");
        assert_eq!(tokens.join(" "), *case);
    }

    // Test with special characters in paths
    let special_chars = [
        "cl.exe /c \"path with spaces.cpp\"",
        "cl.exe /c path-with-dashes.cpp",
        "cl.exe /c path_with_underscores.cpp",
        "cl.exe /c path.with.dots.cpp",
        "cl.exe /c path(with)parens.cpp",
        "cl.exe /c path[with]brackets.cpp",
    ];

    for case in &special_chars {
        let tokens = parser::tokenize_compile_command(case);
        assert!(
            tokens.len() >= 3,
            "Should handle special characters: {}",
            case
        );

        assert!(
            tokens.iter().all(|t| !t.contains('"')),
            "Should remove quotes: {}",
            case
        );
    }
}

/// Checks that extension and directory matching remains case-insensitive.
#[test]
fn test_case_sensitivity() {
    let config = Config::default();

    // Test case insensitive file extension matching
    let case_variants = [
        ("cpp", true),
        ("CPP", true),
        ("Cpp", true),
        ("cPp", true),
        ("c", true),
        ("C", true),
        ("h", true),
        ("H", true),
        ("TXT", false),
        ("txt", false),
        ("Txt", false),
    ];

    for (ext, should_match) in &case_variants {
        let result = parser::should_process_file_extension(
            OsStr::new(ext),
            &config.file_extensions,
        );
        assert_eq!(
            result, *should_match,
            "Case sensitivity test failed for: {}",
            ext
        );
    }

    // Test case insensitive directory exclusion
    let dir_variants = [
        (".git", true),
        (".GIT", true),
        (".Git", true),
        (".gIt", true),
        ("git", false), // No leading dot
        ("GIT", false), // No leading dot
    ];

    for (dir, should_exclude) in &dir_variants {
        let result = parser::should_exclude_directory(
            OsStr::new(dir),
            &config.exclude_directories,
        );
        assert_eq!(
            result, *should_exclude,
            "Directory exclusion test failed for: {}",
            dir
        );
    }
}

/// Runs parser helpers concurrently to detect any hidden synchronization
/// issues.
#[test]
fn test_concurrent_safety() {
    use std::sync::Arc;
    use std::thread;

    let config = Arc::new(Config::default());

    // Test concurrent access to parser functions
    let handles: Vec<_> = (0..10)
        .map(|i| {
            let config = Arc::clone(&config);
            thread::spawn(move || {
                // Each thread performs different operations
                for j in 0..100 {
                    let test_ext = format!("ext{}", (i * 100 + j) % 5);
                    let _ = parser::should_process_file_extension(
                        OsStr::new(test_ext.as_str()),
                        &config.file_extensions,
                    );

                    let test_dir = format!("dir{}", (i * 100 + j) % 3);
                    let _ = parser::should_exclude_directory(
                        OsStr::new(test_dir.as_str()),
                        &config.exclude_directories,
                    );

                    let test_line =
                        format!("cl.exe /c test{}.cpp", i * 100 + j);
                    let _ = parser::tokenize_compile_command(&test_line);
                    let _ = parser::ends_with_cpp_source_file(
                        &test_line,
                        &config.file_extensions,
                    );
                }
            })
        })
        .collect();

    // Wait for all threads to complete
    for handle in handles {
        handle.join().expect("Thread should not panic");
    }
}

/// Exercises tokenization with repeated invocations to catch potential memory
/// leaks or unchecked growth.
#[test]
fn test_memory_usage_patterns() {
    // Test that repeated operations don't cause memory leaks
    for _ in 0..1000 {
        let large_string =
            "cl.exe ".to_owned() + &"/D DEFINE=1 ".repeat(100) + "main.cpp";

        let tokens = parser::tokenize_compile_command(&large_string);
        assert!(!tokens.is_empty());

        assert!(tokens.iter().any(|t| t.contains("cl.exe")));

        drop(tokens);
    }

    // Test with progressively larger inputs
    for size in [10, 100, 1000, 10000] {
        let test_string = "cl.exe /c ".to_owned() + &"A".repeat(size) + ".cpp";

        let result = parser::tokenize_compile_command(&test_string);
        assert_eq!(result.len(), 3); // cl.exe, /c, and the long filename

        assert!(result.iter().all(|t| !t.contains('"')));
    }
}

/// Ensures compile command creation surfaces errors for problematic source
/// paths.
#[test]
fn test_compile_command_creation_errors() {
    let _temp_dir = TempDir::new().unwrap();

    // Test error conditions for compile command creation
    let error_cases = [
        // Root path (no parent)
        #[cfg(windows)]
        "C:\\",
        #[cfg(unix)]
        "/",
        // Non-existent paths are still valid for testing path parsing
        "/non/existent/path/main.cpp",
        "relative/path/main.cpp",
    ];

    for path_str in &error_cases {
        let path = std::path::PathBuf::from(path_str);
        let args = vec![
            OsString::from("cl.exe"),
            OsString::from("/c"),
            OsString::from(path_str),
        ];

        // Test our library function
        let result = compile_commands::create_compile_command(path, args);

        // Root paths should fail (no parent directory)
        if path_str.len() <= 3 {
            // Root paths like "/" or "C:\"
            assert!(result.is_err(), "Root path should fail: {}", path_str);
        }
        // Other paths might succeed or fail depending on structure
    }
}

/// Covers `/Fo` parsing logic and discovery of the flag inside argument lists.
#[test]
fn test_fo_argument_edge_cases() {
    // Test /Fo argument parsing edge cases
    let fo_cases = [
        ("/Fo", true),       // Empty path (technically valid)
        ("/FoDebug/", true), // Normal case
        ("/Fo/very/long/path/to/output/directory/", true), // Long path
        ("/FoRelative/path", true), // Relative path
        ("/Fo.", true),      // Current directory
        ("/Fo..", true),     // Parent directory
        ("/Fo/", true),      // Root directory
    ];

    for (fo_arg, should_succeed) in &fo_cases {
        let result = compile_commands::extract_fo_path(OsStr::new(fo_arg));

        if *should_succeed {
            assert!(
                result.is_ok(),
                "/Fo parsing should succeed for: {}",
                fo_arg
            );
        } else {
            assert!(result.is_err(), "/Fo parsing should fail for: {}", fo_arg);
        }
    }

    // Test finding /Fo in argument lists
    let arg_lists = [
        vec!["cl.exe", "/c", "/FoDebug/", "main.cpp"], // Found
        vec!["cl.exe", "/c", "main.cpp"],              // Not found
        vec!["cl.exe", "/c", "/Zi", "/FoRelease/", "/W3", "main.cpp"], // Found in middle
        vec!["/FoFirst/", "cl.exe", "/c", "main.cpp"], // Found at start
        vec!["cl.exe", "/c", "main.cpp", "/FoLast/"],  // Found at end
    ];

    for args in &arg_lists {
        let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let result = compile_commands::find_fo_argument(&os_args);

        let has_fo = args.iter().any(|arg| arg.starts_with("/Fo"));
        if has_fo {
            assert!(result.is_some(), "Should find /Fo in: {:?}", args);
        } else {
            assert!(result.is_none(), "Should not find /Fo in: {:?}", args);
        }
    }
}
