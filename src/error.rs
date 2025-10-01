//! Error types used throughout the ms2cc library and binary.
//!
//! This module defines the `Ms2ccError` enum, a structured error type that
//! captures rich context for everything from filesystem failures to parsing
//! hiccups. Library code bubbles these values up so higher layers can log or
//! display friendly diagnostics without stringly-typed plumbing.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// Structured error value that covers I/O issues, parsing failures, and
/// validation errors discovered while translating MSBuild logs into
/// `compile_commands.json` entries.
#[derive(Debug, Error)]
pub enum Ms2ccError {
    #[error("path {path:?} is missing a file name component")]
    MissingFileName { path: PathBuf },
    #[error("path {path:?} is missing an extension")]
    MissingExtension { path: PathBuf },
    #[error("path {path:?} is missing a parent directory")]
    MissingParent { path: PathBuf },
    #[error("invalid /Fo argument: {argument}")]
    InvalidFoArgument { argument: String },
    #[error("missing /Fo argument in compile arguments: {arguments:?}")]
    MissingFoArgument { arguments: Vec<OsString> },
    #[error("failed to normalize path {path:?}")]
    PathNormalization { path: PathBuf },
    #[error("encountered unexpected directory entry {path:?}")]
    UnexpectedEntry { path: PathBuf },
    #[error("failed to read log {path:?}: {source}")]
    LogRead {
        #[source]
        source: io::Error,
        path: PathBuf,
    },
    #[error("token vector is empty")]
    EmptyTokenVector,
    #[error("missing trailing source file in arguments: {arguments:?}")]
    MissingTrailingFile { arguments: Vec<OsString> },
    #[error("unexpected line {line} while building compile command {current}")]
    UnexpectedLine { line: String, current: String },
    #[error(
        "failed to resolve source path for {file:?} with arguments {arguments:?}"
    )]
    UnresolvedSourcePath {
        file: PathBuf,
        arguments: Vec<OsString>,
    },
    #[error("I/O error for {path:?}: {source}")]
    Io {
        #[source]
        source: io::Error,
        path: PathBuf,
    },
}

impl Ms2ccError {
    /// Convenience helper that wraps a raw `io::Error` together with the path
    /// that triggered it. Callers use this to collect contextual details before
    /// forwarding the error to the central reporting thread.
    pub fn io_error(source: io::Error, path: impl Into<PathBuf>) -> Self {
        Self::Io {
            source,
            path: path.into(),
        }
    }
}
