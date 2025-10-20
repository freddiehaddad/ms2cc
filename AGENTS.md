# Agent Guidelines for MS2CC

## Build, Lint, and Test Commands

- **Build**: `cargo build` (debug) or `cargo build --release` (optimized)
- **Lint**: `cargo fmt` (format code), `cargo clippy` (lint)
- **Test**: `cargo test` (all tests)
- **Bench**: `cargo bench` (benchmarks)
- **Single test**: `cargo test <test_function_name>` or `cargo test --test <integration_test_file>`
- **Smoke test**: `cargo run -- --input-file path/to/msbuild.log --source-directory path/to/src --pretty-print`

## Code Style Guidelines

### Formatting
- Max line width: 80 characters (configured in `.rustfmt.toml`)
- Use `cargo fmt` to format code automatically

### Naming Conventions
- **Variables/Functions**: `snake_case`
- **Types/Structs/Enums**: `PascalCase`
- **Constants**: `SCREAMING_SNAKE_CASE`
- **Modules**: `snake_case`

### Imports
```rust
// Group imports: std, external crates, local modules
use std::{fs, path::PathBuf};
use clap::Parser;
use serde_json::Value;

mod cli;
mod error;
```

### Error Handling
- Use `Result<T, E>` for fallible operations
- Define custom error types with `thiserror::Error`
- Use descriptive error messages with context
- Prefer early returns with `?` operator

### Documentation
- Use `///` for public API documentation
- Use `//!` for module-level documentation
- Include examples in doc comments where helpful

### Types and Patterns
- Use strong typing: prefer `PathBuf` over `String` for paths
- Use `NonZeroUsize` for thread counts to prevent zero values
- Leverage Serde for serialization with `#[derive(Serialize, Deserialize)]`
- Use builder patterns for complex configuration structs

### Testing
- Use `tempfile::TempDir` for temporary test directories
- Use `assert_cmd` for CLI integration tests
- Write descriptive test function names: `snake_case`
- Test both success and failure cases

### Dependencies
- **CLI**: `clap` with derive macros
- **Serialization**: `serde` with JSON support
- **Error handling**: `thiserror`
- **Threading**: `crossbeam-channel`, `dashmap`
- **Testing**: `assert_cmd`, `tempfile`, `criterion` for benchmarks