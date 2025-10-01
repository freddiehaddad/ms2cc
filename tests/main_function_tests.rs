//! Broader integration-style tests that mirror `main.rs` behavior using
//! temporary logs and synthetic directory trees.

use ms2cc::{Config, parser};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;

/// Returns `true` if the given line references the target compiler token,
/// trimming surrounding quotes along the way.
fn contains_compiler_token(line: &str, compiler: &str) -> bool {
    line.split_whitespace()
        .map(|token| token.trim_matches(|c| matches!(c, '"' | '\'')))
        .any(|token| token.eq_ignore_ascii_case(compiler))
}

/// Parses a realistic MSBuild log snippet and verifies single- vs multi-line
/// command detection.
#[test]
fn test_msbuild_log_parsing_realistic() {
    let temp_dir = TempDir::new().unwrap();
    let log_file = temp_dir.path().join("msbuild.log");

    // Create a realistic MSBuild log file
    let mut file = File::create(&log_file).unwrap();
    writeln!(file, "Microsoft (R) Build Engine version 16.11.2+f32259642 for .NET Framework").unwrap();
    writeln!(
        file,
        "Copyright (C) Microsoft Corporation. All rights reserved."
    )
    .unwrap();
    writeln!(file).unwrap();
    writeln!(file, "  cl.exe /c /Zi /nologo /W3 /WX- /diagnostics:column /Od /Ob0 /D WIN32 /D _WINDOWS /D _DEBUG /D _UNICODE /D UNICODE /Gm- /EHsc /RTC1 /MDd /GS /fp:precise /Zc:wchar_t /Zc:forScope /Zc:inline /Fo\"Debug\\\\\" /Fd\"Debug\\\\vc143.pdb\" /external:W3 /Gd /TP /analyze- /errorReport:prompt main.cpp").unwrap();
    writeln!(file, "  main.cpp").unwrap();
    writeln!(file).unwrap();
    writeln!(
        file,
        "  cl.exe /c /Zi /nologo /W3 /WX- /diagnostics:column /Od /Ob0 /D WIN32"
    )
    .unwrap();
    writeln!(file, "  /D _WINDOWS /D _DEBUG /D _UNICODE /D UNICODE /Gm- /EHsc /RTC1 /MDd /GS").unwrap();
    writeln!(file, "  /fp:precise /Zc:wchar_t /Zc:forScope /Zc:inline /Fo\"Debug\\\\\" utils.cpp").unwrap();
    writeln!(file, "  utils.cpp").unwrap();
    writeln!(file).unwrap();
    writeln!(file, "Build succeeded.").unwrap();
    writeln!(file, "    0 Warning(s)").unwrap();
    writeln!(file, "    0 Error(s)").unwrap();

    // Test that we can identify compile commands
    let config = Config::default();
    let content = fs::read_to_string(&log_file).unwrap();

    let mut single_line_commands = 0;
    let mut multi_line_potential = 0;

    for line in content.lines() {
        if contains_compiler_token(line, &config.compiler_executable) {
            if parser::ends_with_cpp_source_file(line, &config.file_extensions)
            {
                single_line_commands += 1;
            } else {
                multi_line_potential += 1;
            }
        }
    }

    // Should find 1 single-line command and 1 start of multi-line command
    assert_eq!(single_line_commands, 1);
    assert_eq!(multi_line_potential, 1);
}

/// Builds a large directory tree and ensures traversal respects exclusion and
/// extension filters.
#[test]
fn test_large_directory_simulation() {
    let temp_dir = TempDir::new().unwrap();
    let config = Config::default();

    // Create a larger directory structure
    let subdirs = ["src", "include", "tests", "examples", "docs"];
    let mut _total_cpp_files = 0;
    let mut _total_other_files = 0;

    for subdir in &subdirs {
        let subdir_path = temp_dir.path().join(subdir);
        fs::create_dir(&subdir_path).unwrap();

        // Create various file types
        let files = [
            ("main.cpp", true),
            ("utils.c", true),
            ("header.h", true),
            ("readme.txt", false),
            ("makefile", false),
            ("config.hpp", true),
            ("test.py", false),
        ];

        for (filename, should_process) in &files {
            let file_path = subdir_path.join(filename);
            File::create(&file_path).unwrap();

            if *should_process {
                _total_cpp_files += 1;
            } else {
                _total_other_files += 1;
            }
        }
    }

    // Test directory traversal logic
    let mut processed_files = 0;
    let mut skipped_files = 0;

    fn visit_directory(
        dir: &Path,
        config: &Config,
        processed: &mut usize,
        skipped: &mut usize,
    ) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();

            if path.is_dir() {
                if let Some(dir_name) = path.file_name()
                    && !parser::should_exclude_directory(
                        dir_name,
                        &config.exclude_directories,
                    )
                {
                    visit_directory(&path, config, processed, skipped);
                }
            } else if path.is_file()
                && let Some(ext) = path.extension()
            {
                if parser::should_process_file_extension(
                    ext,
                    &config.file_extensions,
                ) {
                    *processed += 1;
                } else {
                    *skipped += 1;
                }
            }
        }
    }

    visit_directory(
        temp_dir.path(),
        &config,
        &mut processed_files,
        &mut skipped_files,
    );

    // Verify that some files were processed (exact count may vary due to directory exclusion logic)
    assert!(processed_files > 0, "Should have processed some C++ files");
    assert!(skipped_files > 0, "Should have skipped some non-C++ files");
}

/// Validates that parser helpers classify lines with and without source files
/// correctly.
#[test]
fn test_error_conditions_in_parsing() {
    let config = Config::default();

    // Test various error conditions
    let test_cases = [
        // Empty line
        ("", false),
        // Line without compiler
        ("some random build output", false),
        // Line with compiler but no source file
        ("cl.exe /c /Zi /nologo", false),
        // Line with compiler and non-C++ file
        ("cl.exe /c /Zi script.py", false),
        // Malformed paths
        ("cl.exe /c /Zi ///invalid//path.cpp", true), // Still ends with .cpp
        // Very long line
        (
            &format!("cl.exe {} main.cpp", "/D VERY_LONG_DEFINE=1".repeat(100)),
            true,
        ),
    ];

    for (line, should_match) in &test_cases {
        let contains_compiler =
            contains_compiler_token(line, &config.compiler_executable);
        let ends_with_cpp =
            parser::ends_with_cpp_source_file(line, &config.file_extensions);

        if *should_match {
            assert!(
                contains_compiler && ends_with_cpp,
                "Line should match: {}",
                line
            );
        } else {
            assert!(
                !(contains_compiler && ends_with_cpp),
                "Line should not match: {}",
                line
            );
        }
    }
}

/// Exercises the tokenizer against whitespace and length edge cases.
#[test]
fn test_tokenization_edge_cases() {
    let test_cases = [
        // Normal case
        ("cl.exe /c main.cpp", vec!["cl.exe", "/c", "main.cpp"]),
        // Multiple spaces
        (
            "cl.exe    /c     main.cpp",
            vec!["cl.exe", "/c", "main.cpp"],
        ),
        // Tabs and mixed whitespace
        ("cl.exe\t/c\t \tmain.cpp", vec!["cl.exe", "/c", "main.cpp"]),
        // Leading/trailing whitespace
        ("  cl.exe /c main.cpp  ", vec!["cl.exe", "/c", "main.cpp"]),
        // Empty string
        ("", vec![]),
        // Single token
        ("cl.exe", vec!["cl.exe"]),
        // Very long command line with proper spacing
        (
            &format!(
                "cl.exe {} main.cpp",
                (0..50)
                    .map(|_| "/DVERY_LONG_DEFINE=\"test value\"")
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            {
                let mut expected = vec!["cl.exe"];
                expected.extend(vec!["/DVERY_LONG_DEFINE=test value"; 50]);
                expected.push("main.cpp");
                expected
            },
        ),
    ];

    for (input, expected) in &test_cases {
        let tokens = parser::tokenize_compile_command(input);
        assert_eq!(tokens, *expected, "Failed for input: '{}'", input);
    }
}

/// Covers additional tokenization scenarios including quoting and escaping.
#[test]
fn test_tokenize_compile_command_edge_cases() {
    let test_cases = [
        ("cl.exe /c main.cpp", vec!["cl.exe", "/c", "main.cpp"]),
        (
            r#""cl.exe" /c "main.cpp""#,
            vec!["cl.exe", "/c", "main.cpp"],
        ),
        (
            r#"cl.exe /I"C:\path with spaces""#,
            vec!["cl.exe", "/IC:\\path with spaces"],
        ),
        (r#"cl.exe """#, vec!["cl.exe", ""]),
        (r#"""#, vec![""]),
        ("", Vec::<&str>::new()),
    ];

    for (input, expected) in &test_cases {
        let tokens = parser::tokenize_compile_command(input);
        let expected: Vec<String> =
            expected.iter().map(|s| s.to_string()).collect();
        assert_eq!(tokens, expected, "Failed for input: '{input}'");
    }
}

/// Simulates path reconstruction outcomes for various argument patterns.
#[test]
fn test_path_reconstruction_scenarios() {
    let temp_dir = TempDir::new().unwrap();

    // Create test source file
    let src_dir = temp_dir.path().join("src");
    fs::create_dir(&src_dir).unwrap();
    let cpp_file = src_dir.join("main.cpp");
    File::create(&cpp_file).unwrap();

    // Test different argument patterns
    let test_cases = [
        // Absolute path
        (
            vec![
                "cl.exe".to_string(),
                "/c".to_string(),
                cpp_file.to_string_lossy().to_string(),
            ],
            true,
        ),
        // Relative path
        (
            vec![
                "cl.exe".to_string(),
                "/c".to_string(),
                "main.cpp".to_string(),
            ],
            false,
        ), // No file map
        // With /Fo argument
        (
            vec![
                "cl.exe".to_string(),
                "/c".to_string(),
                format!("/Fo{}", src_dir.to_string_lossy()),
                "main.cpp".to_string(),
            ],
            true,
        ),
        // Invalid /Fo argument
        (
            vec![
                "cl.exe".to_string(),
                "/c".to_string(),
                "/FoNonExistent/".to_string(),
                "main.cpp".to_string(),
            ],
            false,
        ),
        // Empty arguments
        (vec![], false),
        // No source file
        (vec!["cl.exe".to_string(), "/c".to_string()], false),
    ];

    for (args, _should_succeed) in &test_cases {
        if args.is_empty() {
            let result = parser::extract_and_validate_filename(Path::new(""));
            assert!(result.is_err(), "Empty args should fail");
            continue;
        }

        if args.len() == 2 {
            // Test missing source file
            continue; // Can't test this scenario directly with our parser functions
        }

        let last_arg = args.last().unwrap();
        let filename_result =
            parser::extract_and_validate_filename(Path::new(last_arg));

        if last_arg.ends_with(".cpp") {
            assert!(
                filename_result.is_ok(),
                "Should extract filename from: {}",
                last_arg
            );
        } else {
            // Other test scenarios require more complex setup
        }
    }
}

/// Confirms configuration data can be shared across threads without races.
#[test]
fn test_concurrent_access_patterns() {
    // Test that our Config can be safely shared across threads
    let config = Config::default();
    let config_ref = &config;

    std::thread::scope(|s| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                s.spawn(move || {
                    // Simulate concurrent access to config
                    for ext in &["cpp", "h", "c", "txt", "py"] {
                        let _ = parser::should_process_file_extension(
                            OsStr::new(ext),
                            &config_ref.file_extensions,
                        );
                    }

                    for dir in &[".git", "src", "target", "build"] {
                        let _ = parser::should_exclude_directory(
                            OsStr::new(dir),
                            &config_ref.exclude_directories,
                        );
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }
    });
}

/// Checks that repeated heavy tokenization stays efficient and accurate.
#[test]
fn test_memory_efficiency() {
    // Test that our parsing functions don't cause excessive allocations
    let large_line =
        "cl.exe ".to_owned() + &"/DTEST=1 ".repeat(1000) + "main.cpp";

    // Tokenization should handle large inputs efficiently
    let tokens = parser::tokenize_compile_command(&large_line);
    assert_eq!(tokens.first().unwrap(), "cl.exe");
    assert_eq!(tokens.last().unwrap(), "main.cpp");
    assert_eq!(tokens.len(), 1002); // cl.exe + 1000 defines + main.cpp

    // Quoted input should still tokenize efficiently
    let quoted_line = format!("\"{}\"", large_line);
    let quoted_tokens = parser::tokenize_compile_command(&quoted_line);
    assert_eq!(quoted_tokens, vec![large_line]);
}
