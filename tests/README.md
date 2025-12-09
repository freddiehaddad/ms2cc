# ms2cc Integration Tests

This directory contains integration tests for ms2cc using sanitized real-world MSBuild logs.

## Test Structure

```
tests/
├── fixtures/                             # Test input/output files
│   ├── sequential_build.log              # MSBuild log with sequential compilation
│   ├── sequential_build.expected.json    # Expected compile_commands.json output
│   ├── parallel_build.log                # MSBuild log with parallel build (/m:4)
│   ├── parallel_build.expected.json      # Expected output for parallel build
│   ├── nested_dependencies.log           # MSBuild log with nested project dependencies
│   └── nested_dependencies.expected.json # Expected output for nested dependencies
└── integration_tests.rs                  # Integration test suite
```

## Running Tests

Run all tests (unit + integration):

```powershell
cargo test
```

Run only integration tests:

```powershell
cargo test --test integration_tests
```

Run a specific integration test:

```powershell
cargo test test_sequential_build
cargo test test_parallel_build
cargo test test_nested_dependencies
```

## How to Add New Integration Tests

### Step 1: Obtain and Sanitize MSBuild Log

1. **Capture a real MSBuild log** with `/flp:v=detailed`:

   ```powershell
   msbuild YourProject.sln /t:Build /p:Configuration=Release /flp:v=detailed;logfile=build.log
   ```

2. **Sanitize the log** to remove sensitive information:
   - Replace real project names with generic names (ProjectA, ProjectB, etc.)
   - Replace real paths with generic paths (C:\TestProject, C:\BuildTools, etc.)
   - Replace source file names with generic names (file1.cpp, source1.cpp, etc.)
   - Keep representative compiler flags and PCH patterns (/Yc, /Yu, /Fp, /FI)
   - Remove any certificate paths, usernames, or proprietary code references
   - Simplify compiler version numbers (14.44.35207 → 14.0)

3. **Save sanitized log** to `tests/fixtures/your_test_case.log`

### Step 2: Generate Expected Output

1. **Run ms2cc** on your sanitized log:

   ```powershell
   cargo build --release
   target/release/ms2cc --input-file tests/fixtures/your_test_case.log --output-file tests/fixtures/your_test_case.expected.json
   ```

2. **Verify the output** manually to ensure it's correct:
   - Check that all source files are present
   - Verify paths are absolute
   - Confirm PCH flags are removed (/Yc, /Yu, /Fp)
   - Confirm /FI flags are preserved
   - Ensure no duplicates

### Step 3: Add Test Function

Add a new test function to `tests/integration_tests.rs`:

```rust
#[test]
fn test_your_test_case() {
    // Build the binary
    let build_result = Command::new("cargo")
        .args(&["build"])
        .output()
        .expect("Failed to build ms2cc binary");

    assert!(
        build_result.status.success(),
        "Failed to build ms2cc binary"
    );

    // Run ms2cc on your_test_case.log
    let actual = run_ms2cc("your_test_case.log")
        .expect("Failed to run ms2cc on your_test_case.log");

    // Load expected output
    let expected = load_expected_json("your_test_case.expected.json")
        .expect("Failed to load expected JSON");

    // Validate spec compliance
    validate_spec_compliance(&actual)
        .expect("Actual output violates JSON Compilation Database spec");

    // Validate ms2cc-specific requirements
    validate_ms2cc_specific(&actual)
        .expect("Actual output violates ms2cc-specific requirements");

    // Validate correctness
    validate_correctness(&actual, &expected)
        .expect("Actual output does not match expected");
}
```

### Step 4: Run and Verify

```powershell
cargo test test_your_test_case
```

## What Tests Validate

Each integration test validates:

1. **JSON Compilation Database Spec Compliance**
   - Output is a valid JSON array
   - Each entry has required fields: `directory`, `file`, and either `command` or `arguments`
   - All paths are absolute
   - Files have valid C/C++ extensions (.c, .cpp, .cc, .cxx, .C)

2. **ms2cc-Specific Requirements**
   - All paths use backslashes (Windows style)
   - PCH flags are removed: `/Yc`, `/Yu`, `/Fp`
   - Force include flags are preserved: `/FI`
   - No duplicate file entries

3. **Correctness Against Expected Output**
   - Same number of compilation entries
   - Same set of files compiled
   - Matching directory and command for each file

## Test Fixtures

Current test fixtures cover:

- **sequential_build.log**: 10 files compiled sequentially in a single project
- **parallel_build.log**: 5 files compiled across multiple projects with parallel build
- **nested_dependencies.log**: 3 files with nested project dependencies and PCH usage

## References

- JSON Compilation Database Spec: https://clang.llvm.org/docs/JSONCompilationDatabase.html
