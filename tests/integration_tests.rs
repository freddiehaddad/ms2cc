// Integration tests for ms2cc
// Tests use sanitized real MSBuild logs from tests/fixtures/

use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Helper function to get the path to the ms2cc binary
fn get_binary_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target");
    path.push("debug");
    path.push("ms2cc.exe");
    path
}

/// Helper function to get the path to a test fixture
fn get_fixture_path(fixture_name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("fixtures");
    path.push(fixture_name);
    path
}

/// Run ms2cc with the given input log file and return the generated JSON
fn run_ms2cc(log_file: &str) -> Result<Value, String> {
    let binary = get_binary_path();
    let input = get_fixture_path(log_file);
    let output = get_fixture_path(&format!("{}.test_output.json", log_file));

    // Ensure output doesn't exist from previous run
    let _ = fs::remove_file(&output);

    // Run ms2cc: ms2cc --input-file <log> --output-file <json>
    let result = Command::new(&binary)
        .arg("--input-file")
        .arg(&input)
        .arg("--output-file")
        .arg(&output)
        .arg("--log-level")
        .arg("off")
        .output()
        .map_err(|e| format!("Failed to execute ms2cc: {}", e))?;

    if !result.status.success() {
        return Err(format!(
            "ms2cc failed with status: {}\nstderr: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr)
        ));
    }

    // Read the generated JSON
    let json_str =
        fs::read_to_string(&output).map_err(|e| format!("Failed to read output JSON: {}", e))?;

    // Clean up output file
    let _ = fs::remove_file(&output);

    // Parse JSON
    serde_json::from_str(&json_str).map_err(|e| format!("Failed to parse JSON: {}", e))
}

/// Load expected JSON from fixture file
fn load_expected_json(expected_file: &str) -> Result<Value, String> {
    let path = get_fixture_path(expected_file);
    let json_str =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read expected JSON: {}", e))?;
    serde_json::from_str(&json_str).map_err(|e| format!("Failed to parse expected JSON: {}", e))
}

/// Validate that the JSON follows the Clang JSON Compilation Database spec
/// Spec: https://clang.llvm.org/docs/JSONCompilationDatabase.html
fn validate_spec_compliance(json: &Value) -> Result<(), String> {
    // Check JSON is an array
    let array = json.as_array().ok_or("JSON is not an array")?;

    for (idx, entry) in array.iter().enumerate() {
        let obj = entry
            .as_object()
            .ok_or(format!("Entry {} is not an object", idx))?;

        // Check required fields
        if !obj.contains_key("directory") {
            return Err(format!("Entry {} missing 'directory' field", idx));
        }
        if !obj.contains_key("file") {
            return Err(format!("Entry {} missing 'file' field", idx));
        }

        // Must have either 'command' OR 'arguments' (ms2cc uses 'command')
        if !obj.contains_key("command") && !obj.contains_key("arguments") {
            return Err(format!(
                "Entry {} missing both 'command' and 'arguments' fields",
                idx
            ));
        }

        // Validate 'directory' is a string
        obj.get("directory")
            .and_then(|v| v.as_str())
            .ok_or(format!("Entry {}: 'directory' is not a string", idx))?;

        // Validate 'file' is a string
        let file_str = obj
            .get("file")
            .and_then(|v| v.as_str())
            .ok_or(format!("Entry {}: 'file' is not a string", idx))?;

        // Validate 'command' is a string (if present)
        if let Some(cmd) = obj.get("command") {
            cmd.as_str()
                .ok_or(format!("Entry {}: 'command' is not a string", idx))?;
        }

        // Check file extension is a source file
        let valid_extensions = [".c", ".cpp", ".cc", ".cxx", ".C"];
        let has_valid_ext = valid_extensions.iter().any(|ext| file_str.ends_with(ext));

        if !has_valid_ext {
            return Err(format!(
                "Entry {}: file '{}' does not have a valid C/C++ extension",
                idx, file_str
            ));
        }
    }

    Ok(())
}

/// Validate ms2cc-specific requirements
fn validate_ms2cc_specific(json: &Value) -> Result<(), String> {
    let array = json.as_array().ok_or("JSON is not an array")?;

    for (idx, entry) in array.iter().enumerate() {
        let obj = entry
            .as_object()
            .ok_or(format!("Entry {} is not an object", idx))?;

        let directory = obj
            .get("directory")
            .and_then(|v| v.as_str())
            .ok_or(format!(
                "Entry {}: 'directory' missing or not a string",
                idx
            ))?;

        let file = obj
            .get("file")
            .and_then(|v| v.as_str())
            .ok_or(format!("Entry {}: 'file' missing or not a string", idx))?;

        let command = obj
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or(format!("Entry {}: 'command' missing or not a string", idx))?;

        // Check directory is absolute (starts with drive letter on Windows)
        if !directory.starts_with("C:\\")
            && !directory.starts_with("D:\\")
            && !directory.starts_with("S:\\")
            && !directory.starts_with("E:\\")
        {
            return Err(format!(
                "Entry {}: directory '{}' is not absolute",
                idx, directory
            ));
        }

        // Check file is absolute
        if !file.starts_with("C:\\")
            && !file.starts_with("D:\\")
            && !file.starts_with("S:\\")
            && !file.starts_with("E:\\")
        {
            return Err(format!("Entry {}: file '{}' is not absolute", idx, file));
        }

        // Check paths use backslashes (Windows normalized)
        if directory.contains('/') {
            return Err(format!("Entry {}: directory contains forward slashes", idx));
        }
        if file.contains('/') {
            return Err(format!("Entry {}: file contains forward slashes", idx));
        }

        // Check command contains CL.exe
        if !command.contains("CL.exe") {
            return Err(format!("Entry {}: command does not contain CL.exe", idx));
        }

        // Check PCH flags are removed (/Yc, /Yu, /Fp)
        if command.contains("/Yc") || command.contains("/Yu") || command.contains("/Fp") {
            return Err(format!(
                "Entry {}: command contains PCH flags (/Yc, /Yu, or /Fp)",
                idx
            ));
        }

        // Note: /FI flags should be preserved (not checked here as they may or may not be present)
    }

    Ok(())
}

/// Validate correctness against expected JSON
fn validate_correctness(actual: &Value, expected: &Value) -> Result<(), String> {
    let actual_array = actual.as_array().ok_or("Actual JSON is not an array")?;
    let expected_array = expected.as_array().ok_or("Expected JSON is not an array")?;

    // Check entry count
    if actual_array.len() != expected_array.len() {
        return Err(format!(
            "Entry count mismatch: expected {}, got {}",
            expected_array.len(),
            actual_array.len()
        ));
    }

    // Build sets of files for comparison
    let mut actual_files: Vec<String> = actual_array
        .iter()
        .filter_map(|e| e.get("file").and_then(|f| f.as_str()).map(String::from))
        .collect();
    let mut expected_files: Vec<String> = expected_array
        .iter()
        .filter_map(|e| e.get("file").and_then(|f| f.as_str()).map(String::from))
        .collect();

    actual_files.sort();
    expected_files.sort();

    // Check for duplicate files
    for i in 1..actual_files.len() {
        if actual_files[i] == actual_files[i - 1] {
            return Err(format!(
                "Duplicate file in actual output: {}",
                actual_files[i]
            ));
        }
    }

    // Check file sets match
    if actual_files != expected_files {
        let missing: Vec<_> = expected_files
            .iter()
            .filter(|f| !actual_files.contains(f))
            .collect();
        let extra: Vec<_> = actual_files
            .iter()
            .filter(|f| !expected_files.contains(f))
            .collect();

        let mut msg = String::from("File set mismatch:\n");
        if !missing.is_empty() {
            msg.push_str(&format!("  Missing files: {:?}\n", missing));
        }
        if !extra.is_empty() {
            msg.push_str(&format!("  Extra files: {:?}\n", extra));
        }
        return Err(msg);
    }

    // For each file in expected, find matching entry in actual and compare
    for expected_entry in expected_array {
        let expected_obj = expected_entry
            .as_object()
            .ok_or("Expected entry is not an object")?;
        let expected_file = expected_obj
            .get("file")
            .and_then(|f| f.as_str())
            .ok_or("Expected entry missing file")?;

        // Find matching actual entry
        let actual_entry = actual_array
            .iter()
            .find(|e| {
                e.get("file")
                    .and_then(|f| f.as_str())
                    .map(|f| f == expected_file)
                    .unwrap_or(false)
            })
            .ok_or(format!("No actual entry found for file: {}", expected_file))?;

        let actual_obj = actual_entry
            .as_object()
            .ok_or("Actual entry is not an object")?;

        // Compare directory
        let expected_dir = expected_obj.get("directory").and_then(|d| d.as_str());
        let actual_dir = actual_obj.get("directory").and_then(|d| d.as_str());
        if expected_dir != actual_dir {
            return Err(format!(
                "Directory mismatch for {}:\n  Expected: {:?}\n  Actual: {:?}",
                expected_file, expected_dir, actual_dir
            ));
        }

        // Compare command
        let expected_cmd = expected_obj.get("command").and_then(|c| c.as_str());
        let actual_cmd = actual_obj.get("command").and_then(|c| c.as_str());
        if expected_cmd != actual_cmd {
            return Err(format!(
                "Command mismatch for {}:\n  Expected: {:?}\n  Actual: {:?}",
                expected_file, expected_cmd, actual_cmd
            ));
        }
    }

    Ok(())
}

/// Run ms2cc with arbitrary arguments and return the process output
fn run_ms2cc_raw(args: &[&str]) -> std::process::Output {
    let binary = get_binary_path();
    Command::new(&binary)
        .args(args)
        .output()
        .expect("Failed to execute ms2cc")
}

// ============================================================================
// Integration Tests
// ============================================================================

#[test]
fn test_missing_input_does_not_create_output() {
    let output_path = get_fixture_path("missing_input_test_output.json");

    // Ensure output doesn't exist from previous run
    let _ = fs::remove_file(&output_path);

    let result = run_ms2cc_raw(&[
        "--input-file",
        "nonexistent.log",
        "--output-file",
        output_path.to_str().unwrap(),
        "--log-level",
        "off",
    ]);

    assert!(
        !result.status.success(),
        "ms2cc should fail when input file doesn't exist"
    );
    assert!(
        !output_path.exists(),
        "Output file should not be created when input file doesn't exist"
    );
}

#[test]
fn test_missing_input_does_not_overwrite_existing_output() {
    let output_path = get_fixture_path("existing_output_test.json");
    let sentinel = "existing content that should not be overwritten";

    // Create an existing output file with known content
    fs::write(&output_path, sentinel).expect("Failed to create test output file");

    let result = run_ms2cc_raw(&[
        "--input-file",
        "nonexistent.log",
        "--output-file",
        output_path.to_str().unwrap(),
        "--log-level",
        "off",
    ]);

    assert!(
        !result.status.success(),
        "ms2cc should fail when input file doesn't exist"
    );

    let contents =
        fs::read_to_string(&output_path).expect("Existing output file should still exist");
    assert_eq!(
        contents, sentinel,
        "Existing output file should not be modified on failure"
    );

    // Clean up
    let _ = fs::remove_file(&output_path);
}

#[test]
fn test_sequential_build() {
    // Build the binary first
    let build_result = Command::new("cargo")
        .args(["build"])
        .output()
        .expect("Failed to build ms2cc binary");

    assert!(
        build_result.status.success(),
        "Failed to build ms2cc binary"
    );

    // Run ms2cc on sequential_build.log
    let actual =
        run_ms2cc("sequential_build.log").expect("Failed to run ms2cc on sequential_build.log");

    // Load expected output
    let expected =
        load_expected_json("sequential_build.expected.json").expect("Failed to load expected JSON");

    // Validate spec compliance
    validate_spec_compliance(&actual)
        .expect("Actual output violates JSON Compilation Database spec");

    // Validate ms2cc-specific requirements
    validate_ms2cc_specific(&actual).expect("Actual output violates ms2cc-specific requirements");

    // Validate correctness
    validate_correctness(&actual, &expected).expect("Actual output does not match expected");
}

#[test]
fn test_parallel_build() {
    // Build the binary first
    let build_result = Command::new("cargo")
        .args(["build"])
        .output()
        .expect("Failed to build ms2cc binary");

    assert!(
        build_result.status.success(),
        "Failed to build ms2cc binary"
    );

    // Run ms2cc on parallel_build.log
    let actual =
        run_ms2cc("parallel_build.log").expect("Failed to run ms2cc on parallel_build.log");

    // Load expected output
    let expected =
        load_expected_json("parallel_build.expected.json").expect("Failed to load expected JSON");

    // Validate spec compliance
    validate_spec_compliance(&actual)
        .expect("Actual output violates JSON Compilation Database spec");

    // Validate ms2cc-specific requirements
    validate_ms2cc_specific(&actual).expect("Actual output violates ms2cc-specific requirements");

    // Validate correctness
    validate_correctness(&actual, &expected).expect("Actual output does not match expected");
}

#[test]
fn test_nested_dependencies() {
    // Build the binary first
    let build_result = Command::new("cargo")
        .args(["build"])
        .output()
        .expect("Failed to build ms2cc binary");

    assert!(
        build_result.status.success(),
        "Failed to build ms2cc binary"
    );

    // Run ms2cc on nested_dependencies.log
    let actual = run_ms2cc("nested_dependencies.log")
        .expect("Failed to run ms2cc on nested_dependencies.log");

    // Load expected output
    let expected = load_expected_json("nested_dependencies.expected.json")
        .expect("Failed to load expected JSON");

    // Validate spec compliance
    validate_spec_compliance(&actual)
        .expect("Actual output violates JSON Compilation Database spec");

    // Validate ms2cc-specific requirements
    validate_ms2cc_specific(&actual).expect("Actual output violates ms2cc-specific requirements");

    // Validate correctness
    validate_correctness(&actual, &expected).expect("Actual output does not match expected");
}

// ============================================================================
// Merge Integration Tests
// ============================================================================

#[test]
fn test_merge_preserves_existing_entries() {
    let binary = get_binary_path();
    let input = get_fixture_path("sequential_build.log");
    let output = get_fixture_path("merge_test_output.json");

    // Clean up
    let _ = fs::remove_file(&output);

    // Pre-populate output with an entry that won't appear in the build log
    let pre_existing = serde_json::json!([
        {
            "directory": "C:\\fake\\project",
            "command": "CL.exe /c fake.cpp",
            "file": "C:\\fake\\project\\fake.cpp"
        }
    ]);
    fs::write(&output, serde_json::to_string(&pre_existing).unwrap())
        .expect("Failed to write pre-existing output");

    // Run ms2cc in default merge mode
    let result = Command::new(&binary)
        .arg("--input-file")
        .arg(&input)
        .arg("--output-file")
        .arg(&output)
        .arg("--log-level")
        .arg("off")
        .output()
        .expect("Failed to execute ms2cc");

    assert!(
        result.status.success(),
        "ms2cc failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    let json_str = fs::read_to_string(&output).expect("Failed to read output");
    let result: Value = serde_json::from_str(&json_str).expect("Failed to parse JSON");
    let array = result.as_array().expect("JSON is not an array");

    // Should contain the pre-existing entry plus entries from the build log
    let has_fake = array.iter().any(|e| {
        e.get("file")
            .and_then(|f| f.as_str())
            .map(|f| f == "C:\\fake\\project\\fake.cpp")
            .unwrap_or(false)
    });
    assert!(
        has_fake,
        "Merged output should preserve pre-existing entries"
    );

    let expected =
        load_expected_json("sequential_build.expected.json").expect("Failed to load expected JSON");
    let expected_count = expected.as_array().unwrap().len();
    assert_eq!(
        array.len(),
        expected_count + 1,
        "Merged output should have all build entries plus the pre-existing one"
    );

    // Clean up
    let _ = fs::remove_file(&output);
}

#[test]
fn test_overwrite_replaces_existing_entries() {
    let binary = get_binary_path();
    let input = get_fixture_path("sequential_build.log");
    let output = get_fixture_path("overwrite_test_output.json");

    // Clean up
    let _ = fs::remove_file(&output);

    // Pre-populate output with an extra entry
    let pre_existing = serde_json::json!([
        {
            "directory": "C:\\fake\\project",
            "command": "CL.exe /c fake.cpp",
            "file": "C:\\fake\\project\\fake.cpp"
        }
    ]);
    fs::write(&output, serde_json::to_string(&pre_existing).unwrap())
        .expect("Failed to write pre-existing output");

    // Run ms2cc with --overwrite
    let result = Command::new(&binary)
        .arg("--input-file")
        .arg(&input)
        .arg("--output-file")
        .arg(&output)
        .arg("--overwrite")
        .arg("--log-level")
        .arg("off")
        .output()
        .expect("Failed to execute ms2cc");

    assert!(
        result.status.success(),
        "ms2cc failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    let json_str = fs::read_to_string(&output).expect("Failed to read output");
    let result: Value = serde_json::from_str(&json_str).expect("Failed to parse JSON");
    let array = result.as_array().expect("JSON is not an array");

    // Should NOT contain the pre-existing entry
    let has_fake = array.iter().any(|e| {
        e.get("file")
            .and_then(|f| f.as_str())
            .map(|f| f == "C:\\fake\\project\\fake.cpp")
            .unwrap_or(false)
    });
    assert!(
        !has_fake,
        "Overwrite mode should not preserve pre-existing entries"
    );

    let expected =
        load_expected_json("sequential_build.expected.json").expect("Failed to load expected JSON");
    assert_eq!(
        array.len(),
        expected.as_array().unwrap().len(),
        "Overwrite output should match expected count exactly"
    );

    // Clean up
    let _ = fs::remove_file(&output);
}

#[test]
fn test_merge_updates_matching_entries() {
    let binary = get_binary_path();
    let input = get_fixture_path("sequential_build.log");
    let output = get_fixture_path("merge_update_test_output.json");

    // Clean up
    let _ = fs::remove_file(&output);

    // First, run ms2cc to generate a full database
    let result = Command::new(&binary)
        .arg("--input-file")
        .arg(&input)
        .arg("--output-file")
        .arg(&output)
        .arg("--overwrite")
        .arg("--log-level")
        .arg("off")
        .output()
        .expect("Failed to execute ms2cc");
    assert!(result.status.success());

    let first_json: Value = serde_json::from_str(&fs::read_to_string(&output).unwrap()).unwrap();
    let first_count = first_json.as_array().unwrap().len();

    // Run again in merge mode with the same log — count should be identical
    let result = Command::new(&binary)
        .arg("--input-file")
        .arg(&input)
        .arg("--output-file")
        .arg(&output)
        .arg("--log-level")
        .arg("off")
        .output()
        .expect("Failed to execute ms2cc");
    assert!(result.status.success());

    let second_json: Value = serde_json::from_str(&fs::read_to_string(&output).unwrap()).unwrap();
    let second_count = second_json.as_array().unwrap().len();

    assert_eq!(
        first_count, second_count,
        "Re-merging same log should not change entry count"
    );

    // Clean up
    let _ = fs::remove_file(&output);
}

#[test]
fn test_merge_recovers_from_corrupted_database() {
    let binary = get_binary_path();
    let input = get_fixture_path("sequential_build.log");
    let output = get_fixture_path("corrupted_test_output.json");

    // Clean up
    let _ = fs::remove_file(&output);

    // Write corrupted JSON
    fs::write(&output, "this is not valid json {{{").expect("Failed to write corrupted output");

    // Run ms2cc in default merge mode — should recover gracefully
    let result = Command::new(&binary)
        .arg("--input-file")
        .arg(&input)
        .arg("--output-file")
        .arg(&output)
        .arg("--log-level")
        .arg("off")
        .output()
        .expect("Failed to execute ms2cc");

    assert!(
        result.status.success(),
        "ms2cc should recover from corrupted database: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    let json_str = fs::read_to_string(&output).expect("Failed to read output");
    let result: Value = serde_json::from_str(&json_str).expect("Output should be valid JSON");
    assert!(
        !result.as_array().unwrap().is_empty(),
        "Should have entries from the build log"
    );

    // Clean up
    let _ = fs::remove_file(&output);
}
