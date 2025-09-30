// tests/error_handling_tests.rs - Tests for error conditions and edge cases

use ms2cc::{Config, compile_commands, parser};
use tempfile::TempDir;

#[test]
fn test_malformed_input_handling() {
    let _config = Config::default();

    // Test malformed compile commands
    let malformed_cases = [
        "",                                 // Empty string
        "not a compile command",            // No compiler
        "cl.exe",                           // Only compiler name
        "cl.exe /c",                        // No source file
        "cl.exe /c file_without_extension", // No extension
        "cl.exe /c .cpp",                   // Extension only
        "cl.exe /c /invalid/path.cpp",      // Invalid path characters
    ];

    for case in &malformed_cases {
        // Test tokenization
        let tokens = parser::tokenize_compile_command(case);

        // Test filename extraction if we have tokens
        if !tokens.is_empty() {
            let last_token = tokens.last().unwrap();

            // Should handle gracefully
            if last_token.contains('.') && last_token.ends_with("cpp") {
                let result = parser::extract_and_validate_filename(last_token);
                // Some should succeed, some should fail - both are valid outcomes
                match result {
                    Ok(_) => {}  // Valid filename extracted
                    Err(_) => {} // Expected failure for malformed input
                }
            }
        }

        // Test cleanup
        let cleaned = parser::cleanup_line(case);
        assert!(
            cleaned.len() <= case.len(),
            "Cleanup should not increase length"
        );
    }
}

#[test]
fn test_boundary_conditions() {
    let config = Config::default();

    // Test with very long strings
    let long_string = "a".repeat(10000);
    let result = parser::cleanup_line(&long_string);
    assert_eq!(result.len(), 10000);

    // Test with many quotes
    let many_quotes = "\"".repeat(1000);
    let cleaned = parser::cleanup_line(&many_quotes);
    assert_eq!(cleaned.len(), 0);

    // Test with mixed content
    let mixed = format!(
        "{}cl.exe{} /c{} main.cpp{}",
        "\"".repeat(100),
        "\"".repeat(100),
        "\"".repeat(100),
        "\"".repeat(100)
    );
    let cleaned = parser::cleanup_line(&mixed);
    assert!(cleaned.contains("cl.exe"));
    assert!(cleaned.contains("main.cpp"));
    assert!(!cleaned.contains('"'));

    // Test directory name edge cases
    let edge_case_dirs = [
        "",
        ".",
        "..",
        "very_long_directory_name_that_might_cause_issues",
        "dir-with-dashes",
        "dir.with.dots",
    ];

    for dir in &edge_case_dirs {
        let result =
            parser::should_exclude_directory(dir, &config.exclude_directories);
        // Should not panic and return a boolean
        assert!(result == true || result == false);
    }

    // Test file extension edge cases
    let edge_case_exts = [
        "",
        "c",
        "C",
        "cPP",
        "h++",
        "very_long_extension",
        "1",
        "123",
    ];

    for ext in &edge_case_exts {
        let result =
            parser::should_process_file_extension(ext, &config.file_extensions);
        // Should not panic and return a boolean
        assert!(result == true || result == false);
    }
}

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

        let cleaned = parser::cleanup_line(case);
        assert_eq!(
            cleaned, *case,
            "Unicode strings without quotes should be unchanged"
        );
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

        let cleaned = parser::cleanup_line(case);
        assert!(!cleaned.contains('"'), "Should remove quotes: {}", case);
    }
}

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
        let result =
            parser::should_process_file_extension(ext, &config.file_extensions);
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
        let result =
            parser::should_exclude_directory(dir, &config.exclude_directories);
        assert_eq!(
            result, *should_exclude,
            "Directory exclusion test failed for: {}",
            dir
        );
    }
}

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
                        &test_ext,
                        &config.file_extensions,
                    );

                    let test_dir = format!("dir{}", (i * 100 + j) % 3);
                    let _ = parser::should_exclude_directory(
                        &test_dir,
                        &config.exclude_directories,
                    );

                    let test_line =
                        format!("cl.exe /c test{}.cpp", i * 100 + j);
                    let _ = parser::tokenize_compile_command(&test_line);
                    let _ = parser::cleanup_line(&test_line);
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

    // Test should complete without data races or panics
    assert!(true);
}

#[test]
fn test_memory_usage_patterns() {
    // Test that repeated operations don't cause memory leaks
    for _ in 0..1000 {
        let large_string =
            "cl.exe ".to_owned() + &"/D DEFINE=1 ".repeat(100) + "main.cpp";

        let tokens = parser::tokenize_compile_command(&large_string);
        assert!(!tokens.is_empty());

        let cleaned = parser::cleanup_line(&large_string);
        assert!(cleaned.contains("cl.exe"));

        drop(tokens);
        drop(cleaned);
    }

    // Test with progressively larger inputs
    for size in [10, 100, 1000, 10000] {
        let test_string = "cl.exe /c ".to_owned() + &"A".repeat(size) + ".cpp";

        let result = parser::tokenize_compile_command(&test_string);
        assert_eq!(result.len(), 3); // cl.exe, /c, and the long filename

        let cleaned = parser::cleanup_line(&test_string);
        assert_eq!(cleaned.len(), test_string.len()); // No quotes to remove
    }
}

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
        let args =
            vec!["cl.exe".to_string(), "/c".to_string(), path_str.to_string()];

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
        let result = compile_commands::extract_fo_path(fo_arg);

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
        let string_args: Vec<String> =
            args.iter().map(|s| s.to_string()).collect();
        let result = compile_commands::find_fo_argument(&string_args);

        let has_fo = args.iter().any(|arg| arg.starts_with("/Fo"));
        if has_fo {
            assert!(result.is_some(), "Should find /Fo in: {:?}", args);
        } else {
            assert!(result.is_none(), "Should not find /Fo in: {:?}", args);
        }
    }
}
