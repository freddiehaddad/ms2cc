use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use log::{LevelFilter, debug, error, info, trace, warn};
use regex::Regex;
use simplelog::*;
use std::mem::take;
use std::{
    fs::File,
    io::{BufRead, BufReader, BufWriter},
    path::{Path, PathBuf},
    time::Duration,
};

// ----------------------------------------------------------------------------
// Logging
// ----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl From<LogLevel> for LevelFilter {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Off => LevelFilter::Off,
            LogLevel::Error => LevelFilter::Error,
            LogLevel::Warn => LevelFilter::Warn,
            LogLevel::Info => LevelFilter::Info,
            LogLevel::Debug => LevelFilter::Debug,
            LogLevel::Trace => LevelFilter::Trace,
        }
    }
}

// ----------------------------------------------------------------------------
// Command-line arguments
// ----------------------------------------------------------------------------

const PACKAGE_DESCRIPTION: &str = env!("CARGO_PKG_DESCRIPTION");
const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(version, about=PACKAGE_DESCRIPTION)]
struct Args {
    /// Full path to msbuild.log file
    #[arg(short = 'i', long, default_value = "msbuild.log")]
    input_file: PathBuf,

    /// Path to output compile_commands.json file
    #[arg(short = 'o', long, default_value = "compile_commands.json")]
    output_file: PathBuf,

    /// Logging level
    #[arg(short = 'l', long, value_enum, default_value = "info")]
    log_level: LogLevel,

    /// Pretty-print JSON output
    #[arg(short = 'p', long, default_value = "false")]
    pretty_print: bool,

    /// Disable progress bar output
    #[arg(long, default_value = "false")]
    no_progress: bool,
}

// ----------------------------------------------------------------------------
// Data Structures
// ----------------------------------------------------------------------------

/// Context for the current project being processed
#[derive(Debug, Clone)]
struct ProjectContext {
    /// Full path to the project file
    project_path: PathBuf,
    /// Directory containing the project file (for resolving relative paths)
    project_dir: PathBuf,
}

/// Represents a single compilation command entry in compile_commands.json
#[derive(Debug, Clone, serde::Serialize)]
struct CompileCommand {
    /// The working directory of the compilation
    directory: String,
    /// The compile command as a single string
    command: String,
    /// The main translation unit source processed by this command
    file: String,
}

/// State tracking for MSBuild log processing
#[derive(Debug)]
struct ProcessingState {
    /// Maps output prefix (e.g., "7>") to the project being built
    prefix_to_project: std::collections::HashMap<u32, ProjectContext>,
    /// Current project context (for sequential builds or fallback)
    current_project: Option<ProjectContext>,
    /// Current output prefix being processed
    current_prefix: Option<u32>,
    /// Total number of compile commands found
    command_count: usize,
}

impl ProcessingState {
    fn new() -> Self {
        Self {
            prefix_to_project: std::collections::HashMap::new(),
            current_project: None,
            current_prefix: None,
            command_count: 0,
        }
    }

    /// Get the active project context based on current prefix or fallback
    fn get_active_project(&self) -> Option<&ProjectContext> {
        if let Some(prefix) = self.current_prefix {
            // Try prefix-aware mapping first (parallel builds)
            self.prefix_to_project
                .get(&prefix)
                .or(self.current_project.as_ref())
        } else {
            // Sequential build: use current_project
            self.current_project.as_ref()
        }
    }
}

/// Bundle of compiled regex patterns for log parsing
struct LogPatterns {
    node_prefix: Regex,
    project_on_node: Regex,
    nested_project: Regex,
    from_project: Regex,
    compile_command: Regex,
}

impl LogPatterns {
    fn new() -> Result<Self> {
        Ok(Self {
            node_prefix: node_prefix_pattern()?,
            project_on_node: project_on_node_pattern()?,
            nested_project: nested_project_pattern()?,
            from_project: from_project_pattern()?,
            compile_command: compile_command_pattern()?,
        })
    }
}

// ----------------------------------------------------------------------------
// Command Line Parsing
// ----------------------------------------------------------------------------

/// Tokenize a command line respecting quoted strings
/// Implements state machine: NORMAL -> IN_QUOTE -> NORMAL
fn tokenize_command_line(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current_token = String::new();
    let mut in_quotes = false;

    for ch in line.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current_token.push(ch);
            }
            ' ' | '\t' if !in_quotes => {
                if !current_token.is_empty() {
                    tokens.push(take(&mut current_token));
                }
            }
            _ => {
                current_token.push(ch);
            }
        }
    }

    if !current_token.is_empty() {
        tokens.push(current_token);
    }

    tokens
}

/// Check if a flag should be filtered out (PCH-related)
fn should_filter_flag(flag: &str) -> bool {
    let flag_upper = flag.to_uppercase();
    // Strip PCH flags: /Yc, /Yu, /Fp<path>
    // Keep /FI (force include) - clangd supports this as -include
    // Keep /fp:<model> (floating-point model) - has colon, different from /Fp (PCH)

    if flag_upper.starts_with("/YC") || flag_upper.starts_with("/YU") {
        return true;
    }

    // Check for /Fp (PCH file) but NOT /fp: (floating-point model)
    // After uppercasing:
    //   /fp:precise → /FP:PRECISE (floating-point, keep it)
    //   /Fp"file.pch" → /FP"FILE.PCH" (PCH, filter it)
    // The discriminator is the colon!
    if flag_upper.starts_with("/FP") && !flag_upper.starts_with("/FP:") {
        return true;
    }

    false
}

/// Check if a token is a source file (.c, .cpp, .cc, .cxx)
fn is_source_file(token: &str) -> bool {
    // Remove quotes if present
    let clean_token = token.trim_matches('"');
    let token_lower = clean_token.to_lowercase();
    token_lower.ends_with(".c")
        || token_lower.ends_with(".cpp")
        || token_lower.ends_with(".cc")
        || token_lower.ends_with(".cxx")
}

/// Normalize a path by rebuilding it from components
/// This eliminates double backslashes, redundant separators, and other path anomalies
fn normalize_path(path: &Path) -> PathBuf {
    path.components().collect()
}

/// Convert a PathBuf to a normalized string representation
fn path_to_normalized_string(path: &Path) -> String {
    normalize_path(path).display().to_string()
}

/// Clean include path by removing trailing backslashes that can cause
/// quote-escaping issues in clangd's command-line parser
///
/// Examples:
///   /I"C:\path\to\dir\\" -> /I"C:\path\to\dir"
///   /I"C:\path\to\dir"   -> /I"C:\path\to\dir" (unchanged)
///   /IC:\path\to\dir\    -> /IC:\path\to\dir (unquoted form)
fn clean_include_path(flag: &str) -> String {
    // Check if this is an include flag
    let flag_upper = flag.to_uppercase();
    if !flag_upper.starts_with("/I") {
        return flag.to_string();
    }

    // Pattern: /I followed by optional quotes, path, optional trailing backslashes, optional closing quote
    // Cases to handle:
    //   /I"C:\path\\"    -> /I"C:\path"
    //   /IC:\path\       -> /IC:\path
    //   /I"C:\path"      -> /I"C:\path" (no change)

    if flag.starts_with("/I\"") || flag.starts_with("/i\"") {
        // Quoted path: /I"path\\"
        if let Some(end_quote_pos) = flag.rfind('"')
            && end_quote_pos > 3
        {
            // More than just /I"<quote>
            let prefix = &flag[..3]; // /I"
            let path = &flag[3..end_quote_pos]; // The actual path
            let suffix = &flag[end_quote_pos..]; // Closing "

            // Remove trailing backslashes from path
            // But preserve at least one backslash if it's a root path like "C:\"
            let cleaned_path = if path.len() == 3 && path.ends_with(":\\") {
                // Root path like "C:\" - keep it
                path
            } else {
                path.trim_end_matches('\\')
            };

            return format!("{}{}{}", prefix, cleaned_path, suffix);
        }
    } else if flag.len() > 2 {
        // Unquoted path: /Ipath\
        let prefix = &flag[..2]; // /I
        let path = &flag[2..]; // The path

        // Remove trailing backslashes
        // But preserve at least one backslash if it's a root path like "C:\"
        let cleaned_path = if path.len() == 3 && path.ends_with(":\\") {
            // Root path like "C:\" - keep it
            path
        } else {
            path.trim_end_matches('\\')
        };

        return format!("{}{}", prefix, cleaned_path);
    }

    // Fallback: return as-is if pattern doesn't match
    flag.to_string()
}

/// Resolve source file path to absolute path
fn resolve_source_file_path(source_file: &str, working_directory: &Path) -> PathBuf {
    let file_path = PathBuf::from(source_file.trim_matches('"'));

    if file_path.is_absolute() {
        return file_path;
    }

    // Resolve relative to working directory
    working_directory.join(&file_path)
}

/// Parse a CL.exe command line and extract compile commands
/// Returns a vector of CompileCommand (one per source file)
fn parse_cl_command(
    line: &str,
    project_ctx: &ProjectContext,
    line_number: usize,
) -> Result<Vec<CompileCommand>> {
    // Extract the full CL.exe path using regex BEFORE tokenization
    // This handles both quoted and unquoted paths with spaces:
    //   Quoted: "C:\Program Files\...\CL.exe"
    //   Unquoted: C:\Program Files\Microsoft Visual Studio\...\CL.exe
    // Pattern matches from drive letter to CL.exe, handling spaces in between
    let cl_exe_regex = regex::Regex::new(r#"(?i)([A-Z]:[^\r\n]*?\\CL\.exe|"[^"]*\\CL\.exe")"#)
        .context("Failed to compile CL.exe regex")?;

    let cl_exe_match = cl_exe_regex
        .find(line)
        .context("CL.exe not found in command line")?
        .as_str();

    // Remove quotes if present
    let cl_exe_path = cl_exe_match.trim_matches('"').to_string();

    let tokens = tokenize_command_line(line);

    // Find CL.exe position in tokens to know where arguments start
    let cl_exe_pos = tokens
        .iter()
        .position(|t| t.to_uppercase().contains("CL.EXE"))
        .context("CL.exe not found in command line")?;

    // Separate source files from flags
    let mut source_files = Vec::new();
    let mut filtered_args = Vec::new();

    // Extract tokens (everything after CL.exe)
    for token in tokens.into_iter().skip(cl_exe_pos + 1) {
        if is_source_file(&token) {
            source_files.push(token);
        } else if !should_filter_flag(&token) {
            // Clean include paths to remove trailing backslashes
            let cleaned_token = clean_include_path(&token);
            filtered_args.push(cleaned_token);
        } else {
            trace!("Filtered PCH flag at line {}: {}", line_number, token);
        }
    }

    if source_files.is_empty() {
        warn!(
            "No source files found in CL.exe command at line {} for project {}",
            line_number,
            project_ctx.project_path.display()
        );
        return Ok(Vec::new());
    }

    // Create one CompileCommand per source file
    let mut commands = Vec::new();

    // Build the base command string once (combines CL.exe path + filtered args)
    let base_command = {
        let cl_exe_token = if cl_exe_path.contains(' ') {
            format!("\"{}\"", cl_exe_path)
        } else {
            cl_exe_path
        };
        let mut parts = vec![cl_exe_token];
        parts.extend(filtered_args);
        parts.join(" ")
    };

    for source_file in source_files {
        // Resolve source file to absolute path
        let absolute_file_path = resolve_source_file_path(&source_file, &project_ctx.project_dir);

        // Normalize paths to eliminate double backslashes and other anomalies
        let normalized_file = path_to_normalized_string(&absolute_file_path);
        let normalized_directory = path_to_normalized_string(&project_ctx.project_dir);

        // Reconstruct command with base command + normalized absolute source file path
        let command = format!("{} \"{}\"", base_command, normalized_file);

        commands.push(CompileCommand {
            directory: normalized_directory,
            command,
            file: normalized_file,
        });
    }

    debug!(
        "Parsed {} compile command(s) from line {} for project {}",
        commands.len(),
        line_number,
        project_ctx.project_path.display()
    );

    Ok(commands)
}

// ----------------------------------------------------------------------------
// Regular Expression Patterns
// ----------------------------------------------------------------------------

/// Pattern to match node prefix (e.g., "7>" or "7:2>" at start of line)
/// Used to track the current build node in parallel builds
/// Handles both simple prefixes (7>) and multi-instance prefixes (7:2>)
fn node_prefix_pattern() -> Result<Regex> {
    let pattern = r"^\s*(\d+)(?::\d+)?>";
    debug!("Compiling node prefix regex: {}", pattern);
    Regex::new(pattern).context("Failed to compile node prefix regex")
}

/// Pattern to match "Project X on node N" (parallel builds)
/// Example: 5>Project "S:\Acme\...\Project.vcxproj" on node 4 (Build target(s)).
/// Also handles multi-instance prefixes like 5:2>Project ...
/// Captures the OUTPUT PREFIX (5 or 5:2) and PROJECT PATH, not the physical node number
fn project_on_node_pattern() -> Result<Regex> {
    let pattern = r#"^\s*(\d+)(?::\d+)?>Project "([^"]+\.vcxproj)" on node \d+"#;
    debug!("Compiling project-on-node regex: {}", pattern);
    Regex::new(pattern).context("Failed to compile project-on-node regex")
}

/// Pattern to match nested "Project X is building Y on node N" (parallel builds with dependencies)
/// Example: 44>Project "Parent.proj" (44) is building "Child.vcxproj" (54) on node 13 (default targets).
/// Also handles multi-instance notation: 44:2>Project "..." (44:2) is building "..." (54:3) on node 13
/// Captures the CHILD PROJECT PATH and CHILD OUTPUT PREFIX (base number only, e.g., 54 from 54:3)
fn nested_project_pattern() -> Result<Regex> {
    let pattern = r#"^\s*\d+(?::\d+)?>Project "[^"]*" \([^\)]+\) is building "([^"]+\.vcxproj)" \((\d+)(?::\d+)?\) on node \d+"#;
    debug!("Compiling nested-project regex: {}", pattern);
    Regex::new(pattern).context("Failed to compile nested-project regex")
}

/// Pattern to match "from project X" (sequential builds)
/// Example: Target "ClCompile" ... from project "C:\...\Project.vcxproj"
fn from_project_pattern() -> Result<Regex> {
    let pattern = r#"from project "([^"]+\.vcxproj)""#;
    debug!("Compiling from-project regex: {}", pattern);
    Regex::new(pattern).context("Failed to compile from-project regex")
}

/// Pattern to match CL.exe compilation commands
/// Matches lines containing CL.exe followed by arguments
fn compile_command_pattern() -> Result<Regex> {
    let pattern = r"^\s+.*CL\.exe\s";
    debug!("Compiling CL.exe command regex: {}", pattern);
    Regex::new(pattern).context("Failed to compile CL.exe command regex")
}

// ----------------------------------------------------------------------------
// Log Processing Helper Functions
// ----------------------------------------------------------------------------

/// Setup and configure the progress bar for reading the build log
fn setup_read_progress_bar(
    show_progress: bool,
    file_size: u64,
    multi: &MultiProgress,
) -> Result<ProgressBar> {
    if show_progress {
        let pb = multi.add(ProgressBar::new(file_size));
        pb.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} {msg}")?
                .progress_chars("=> "),
        );
        pb.set_message("Processing build log...");
        Ok(pb)
    } else {
        Ok(ProgressBar::hidden())
    }
}

/// Setup and configure the spinner progress bar for writing output
fn setup_write_progress_bar(show_progress: bool, multi: &MultiProgress) -> Result<ProgressBar> {
    if show_progress {
        let pb = multi.add(ProgressBar::new_spinner());
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("[{elapsed_precise}] {spinner:.cyan} {bytes} {msg}")?
                .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
        );
        pb.set_message("Writing output...");
        pb.enable_steady_tick(Duration::from_millis(100));
        Ok(pb)
    } else {
        Ok(ProgressBar::hidden())
    }
}

/// Finalize processing and log summary information
fn finalize_processing(state: &ProcessingState, pb: ProgressBar) {
    pb.finish_and_clear();

    info!(
        "Processing complete: {} unique output prefixes, {} compile commands",
        state.prefix_to_project.len(),
        state.command_count
    );

    if state.prefix_to_project.is_empty() && state.current_project.is_none() {
        warn!(
            "No projects found in build log - ensure MSBuild was run with /v:detailed or /v:diagnostic"
        );
    }

    if !state.prefix_to_project.is_empty() && state.command_count == 0 {
        warn!(
            "Found {} output prefixes with projects but no compile commands - build log may be incomplete",
            state.prefix_to_project.len()
        );
    }
}

/// Handle node prefix pattern (e.g., "7>")
fn handle_node_prefix(
    line: &str,
    pattern: &Regex,
    state: &mut ProcessingState,
    line_number: usize,
) {
    if let Some(caps) = pattern.captures(line)
        && let Ok(prefix_num) = caps[1].parse::<u32>()
    {
        state.current_prefix = Some(prefix_num);
        trace!(
            "Switched to output prefix {} at line {}",
            prefix_num, line_number
        );
    }
}

/// Handle "Project X on node N" pattern (parallel builds)
fn handle_project_on_node(
    line: &str,
    pattern: &Regex,
    state: &mut ProcessingState,
    line_number: usize,
) -> Result<()> {
    if let Some(caps) = pattern.captures(line) {
        let prefix_num = caps[1]
            .parse::<u32>()
            .context("Failed to parse output prefix")?;
        let project_path = PathBuf::from(&caps[2]);
        let project_dir = project_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        let ctx = ProjectContext {
            project_path: project_path.clone(),
            project_dir,
        };

        trace!(
            "Assigned project {} to output prefix {} at line {}",
            project_path.display(),
            prefix_num,
            line_number
        );

        state.prefix_to_project.insert(prefix_num, ctx.clone());
        // Also update current_project as fallback for sequential builds
        state.current_project = Some(ctx);
    }
    Ok(())
}

/// Handle nested "Project X is building Y on node N" pattern
fn handle_nested_project(
    line: &str,
    pattern: &Regex,
    state: &mut ProcessingState,
    line_number: usize,
) -> Result<()> {
    if let Some(caps) = pattern.captures(line) {
        let project_path = PathBuf::from(&caps[1]);
        let prefix_num = caps[2]
            .parse::<u32>()
            .context("Failed to parse nested project output prefix")?;
        let project_dir = project_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        let ctx = ProjectContext {
            project_path: project_path.clone(),
            project_dir,
        };

        trace!(
            "Assigned nested project {} to output prefix {} at line {}",
            project_path.display(),
            prefix_num,
            line_number
        );

        state.prefix_to_project.insert(prefix_num, ctx.clone());
        // Also update current_project as fallback
        state.current_project = Some(ctx);
    }
    Ok(())
}

/// Handle "from project X" pattern (sequential builds)
fn handle_from_project(
    line: &str,
    pattern: &Regex,
    state: &mut ProcessingState,
    line_number: usize,
) {
    if let Some(caps) = pattern.captures(line) {
        let project_path = PathBuf::from(&caps[1]);
        let project_dir = project_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        let ctx = ProjectContext {
            project_path: project_path.clone(),
            project_dir,
        };

        trace!(
            "Set current project to {} at line {}",
            project_path.display(),
            line_number
        );

        state.current_project = Some(ctx);
    }
}

/// Handle CL.exe compilation command
fn handle_cl_command(
    line: &str,
    pattern: &Regex,
    state: &ProcessingState,
    line_number: usize,
) -> Result<Vec<CompileCommand>> {
    if !pattern.is_match(line) {
        return Ok(Vec::new());
    }

    // Determine which project this command belongs to
    let project_ctx = state.get_active_project();

    if let Some(proj_ctx) = project_ctx {
        match parse_cl_command(line, proj_ctx, line_number) {
            Ok(commands) => Ok(commands),
            Err(e) => {
                error!(
                    "Failed to parse CL.exe command at line {}: {:?}",
                    line_number, e
                );
                Ok(Vec::new())
            }
        }
    } else {
        warn!(
            "Found CL.exe command at line {} but no project context available",
            line_number
        );
        Ok(Vec::new())
    }
}

/// Process the MSBuild log file. Tracks projects per output prefix for parallel
/// builds and uses context markers for sequential builds
fn process_msbuild_log(
    input_file: &Path,
    patterns: LogPatterns,
    show_progress: bool,
    multi: &MultiProgress,
) -> Result<Vec<CompileCommand>> {
    let mut compile_commands = Vec::new();
    let mut state = ProcessingState::new();

    info!("Starting MSBuild log processing");

    // Open file and get size for progress tracking
    let file = File::open(input_file)
        .with_context(|| format!("Failed to open input file: {}", input_file.display()))?;
    let file_size = file.metadata()?.len();

    // Create progress bar
    let pb = setup_read_progress_bar(show_progress, file_size, multi)?;

    // Wrap file with progress tracking
    let progress_reader = pb.wrap_read(file);
    let input = BufReader::new(progress_reader);

    // Single-pass processing
    for (index, line_result) in input.lines().enumerate() {
        let line_number = index + 1;

        let line = match line_result {
            Ok(l) => l,
            Err(e) => {
                warn!("Failed to read line {}: {:?}", line_number, e);
                continue;
            }
        };

        // Process each pattern type
        handle_node_prefix(&line, &patterns.node_prefix, &mut state, line_number);

        if let Err(e) =
            handle_project_on_node(&line, &patterns.project_on_node, &mut state, line_number)
        {
            error!(
                "Failed to process project-on-node at line {}: {:?}",
                line_number, e
            );
        }

        if let Err(e) =
            handle_nested_project(&line, &patterns.nested_project, &mut state, line_number)
        {
            error!(
                "Failed to process nested project at line {}: {:?}",
                line_number, e
            );
        }

        handle_from_project(&line, &patterns.from_project, &mut state, line_number);

        match handle_cl_command(&line, &patterns.compile_command, &state, line_number) {
            Ok(commands) => {
                state.command_count += commands.len();
                compile_commands.extend(commands);
            }
            Err(e) => {
                error!(
                    "Failed to handle CL command at line {}: {:?}",
                    line_number, e
                );
            }
        }
    }

    finalize_processing(&state, pb);

    Ok(compile_commands)
}

fn open_output_file(path: &Path) -> Result<BufWriter<File>> {
    debug!("Opening output file: {}", path.display());
    let file = File::create(path)
        .with_context(|| format!("Failed to create output file: {}", path.display()))?;
    Ok(BufWriter::new(file))
}

fn run() -> Result<()> {
    let args = Args::parse();

    // Determine if progress bar should be shown
    // Disable only if --no-progress flag is set or output is not a TTY
    let show_progress = !args.no_progress && atty::is(atty::Stream::Stderr);

    // Create MultiProgress for coordinating progress bars and logging
    let multi = MultiProgress::new();

    // Initialize logging with indicatif-log-bridge
    let config = ConfigBuilder::new()
        .set_target_level(LevelFilter::Off)
        .set_thread_level(LevelFilter::Off)
        .build();

    let log_level_filter: LevelFilter = args.log_level.into();

    let logger = TermLogger::new(
        log_level_filter,
        config,
        TerminalMode::Mixed,
        ColorChoice::Auto,
    );

    indicatif_log_bridge::LogWrapper::new(multi.clone(), logger)
        .try_init()
        .context("Failed to initialize logging")?;

    info!("ms2cc v{} - {}", PACKAGE_VERSION, PACKAGE_DESCRIPTION);

    // Open output file early in case of an error.
    let output = open_output_file(&args.output_file)?;

    // Process the MSBuild log file
    let patterns = LogPatterns::new()?;
    let compile_commands = process_msbuild_log(&args.input_file, patterns, show_progress, &multi)?;

    // Write JSON output
    info!(
        "Writing {} commands to {}",
        compile_commands.len(),
        args.output_file.display()
    );

    // Create progress spinner for write operation if enabled
    let write_pb = setup_write_progress_bar(show_progress, &multi)?;

    // Wrap output with progress tracking
    let progress_writer = write_pb.wrap_write(output);

    if args.pretty_print {
        serde_json::to_writer_pretty(progress_writer, &compile_commands)
            .context("Failed to write JSON output")?;
    } else {
        serde_json::to_writer(progress_writer, &compile_commands)
            .context("Failed to write JSON output")?;
    }

    write_pb.finish_and_clear();

    info!("Finished");

    Ok(())
}

// ----------------------------------------------------------------------------
// Main entry point
// ----------------------------------------------------------------------------

fn main() -> Result<()> {
    if let Err(e) = run() {
        error!("Application error: {:?}", e);
        std::process::exit(1);
    };

    Ok(())
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ----------------------------------------------------------------------------
    // Tests for regex patterns
    // ----------------------------------------------------------------------------

    #[test]
    fn test_node_prefix_pattern() {
        let re = node_prefix_pattern().unwrap();

        assert!(re.is_match("4>Project ..."));
        assert!(re.is_match("  7>Something"));
        assert!(re.is_match("123>Build"));
        assert!(!re.is_match("Project without prefix"));

        // Extract node number
        let caps = re.captures("  4>Project").unwrap();
        assert_eq!(&caps[1], "4");

        // Test multi-instance prefix notation (e.g., "53:20>")
        assert!(re.is_match("53:20>Project ..."));
        assert!(re.is_match("  7:2>Something"));
        let caps = re.captures("53:20>ClCompile").unwrap();
        assert_eq!(&caps[1], "53"); // Should capture base number only
    }

    #[test]
    fn test_project_on_node_pattern() {
        let re = project_on_node_pattern().unwrap();

        let line1 = r#"4>Project "C:\path\to\project.vcxproj" on node 3 (Build target(s))."#;
        let caps = re.captures(line1).expect("Should match");
        assert_eq!(&caps[1], "4"); // Output prefix
        assert_eq!(&caps[2], r#"C:\path\to\project.vcxproj"#); // Project path

        let line2 = r#"  7>Project "S:\My Project\test.vcxproj" on node 12 (default targets)."#;
        let caps = re.captures(line2).expect("Should match path with spaces");
        assert_eq!(&caps[1], "7"); // Output prefix
        assert_eq!(&caps[2], r#"S:\My Project\test.vcxproj"#); // Project path

        // Test multi-instance prefix notation
        let line3 = r#"  53:20>Project "S:\Azure\test.vcxproj" on node 30 (default targets)."#;
        let caps = re
            .captures(line3)
            .expect("Should match multi-instance prefix");
        assert_eq!(&caps[1], "53"); // Base prefix number
        assert_eq!(&caps[2], r#"S:\Azure\test.vcxproj"#); // Project path
    }

    #[test]
    fn test_nested_project_pattern() {
        let re = nested_project_pattern().unwrap();

        let line1 = r#"    44>Project "S:\Acme\corp\src\foo\baz.proj" (44) is building "S:\Acme\corp\src\foo\bar.vcxproj" (54) on node 13 (default targets)."#;
        let caps = re
            .captures(line1)
            .expect("Should match nested project pattern");
        assert_eq!(&caps[1], r#"S:\Acme\corp\src\foo\bar.vcxproj"#); // Child project path
        assert_eq!(&caps[2], "54"); // Child output prefix

        // Another example with spaces
        let line2 = r#"  10>Project "C:\Parent.proj" (10) is building "C:\My Projects\Child.vcxproj" (25) on node 5 (Build target(s))."#;
        let caps = re.captures(line2).expect("Should match nested with spaces");
        assert_eq!(&caps[1], r#"C:\My Projects\Child.vcxproj"#); // Child project path
        assert_eq!(&caps[2], "25"); // Child output prefix

        // Test multi-instance prefix notation
        let line3 = r#" 53:20>Project "S:\Azure\Parent.csproj" (53:20) is building "S:\Azure\XStoreUlsNative.vcxproj" (246) on node 30 (default targets)."#;
        let caps = re
            .captures(line3)
            .expect("Should match multi-instance parent prefix");
        assert_eq!(&caps[1], r#"S:\Azure\XStoreUlsNative.vcxproj"#); // Child project path
        assert_eq!(&caps[2], "246"); // Child output prefix (base number)

        // Test multi-instance for both parent and child
        let line4 = r#" 264:105>Project "S:\Azure\Parent.vcxproj" (264:105) is building "S:\Azure\Child.vcxproj" (266:5) on node 28 (BuiltProjectOutputGroup target(s))."#;
        let caps = re
            .captures(line4)
            .expect("Should match multi-instance both prefixes");
        assert_eq!(&caps[1], r#"S:\Azure\Child.vcxproj"#); // Child project path
        assert_eq!(&caps[2], "266"); // Child output prefix (base number only)
    }

    #[test]
    fn test_from_project_pattern() {
        let re = from_project_pattern().unwrap();

        let line1 = r#"Target "ClCompile" from project "C:\path\to\project.vcxproj""#;
        let caps = re.captures(line1).expect("Should match");
        assert_eq!(&caps[1], r#"C:\path\to\project.vcxproj"#);

        let line2 = r#"  Some text from project "D:\My Projects\test.vcxproj" more text"#;
        let caps = re.captures(line2).expect("Should match path with spaces");
        assert_eq!(&caps[1], r#"D:\My Projects\test.vcxproj"#);
    }

    #[test]
    fn test_cl_exe_regex() {
        let re = compile_command_pattern().unwrap();

        assert!(re.is_match(r#"  CL.exe /c /I"include" main.cpp"#));
        assert!(re.is_match(r#"    C:\Program Files\MSVC\bin\CL.exe /nologo"#));
        assert!(!re.is_match(r#"CL.exe"#)); // No space after CL.exe
        assert!(!re.is_match(r#"Link.exe /OUT:test.exe"#));
    }

    // ----------------------------------------------------------------------------
    // Tests for argument tokenization and command parsing
    // ----------------------------------------------------------------------------

    #[test]
    fn test_tokenize_simple() {
        let tokens = tokenize_command_line(r#"cl.exe /c main.cpp"#);
        assert_eq!(tokens, vec!["cl.exe", "/c", "main.cpp"]);
    }

    #[test]
    fn test_tokenize_quoted() {
        let tokens = tokenize_command_line(r#"cl.exe /I"C:\Program Files\include" main.cpp"#);
        assert_eq!(
            tokens,
            vec!["cl.exe", r#"/I"C:\Program Files\include""#, "main.cpp"]
        );
    }

    #[test]
    fn test_tokenize_multiple_spaces() {
        let tokens = tokenize_command_line(r#"cl.exe   /c    main.cpp"#);
        assert_eq!(tokens, vec!["cl.exe", "/c", "main.cpp"]);
    }

    #[test]
    fn test_is_source_file() {
        assert!(is_source_file("main.cpp"));
        assert!(is_source_file("test.c"));
        assert!(is_source_file("code.cc"));
        assert!(is_source_file("file.cxx"));
        assert!(is_source_file("FILE.CPP")); // Case insensitive
        assert!(!is_source_file("header.h"));
        assert!(!is_source_file("lib.obj"));
    }

    #[test]
    fn test_should_filter_flag() {
        // Should filter PCH flags
        assert!(should_filter_flag("/Yc"));
        assert!(should_filter_flag("/YcStdAfx.h"));
        assert!(should_filter_flag("/Yu"));
        assert!(should_filter_flag("/YuPrecompiled.h"));
        assert!(should_filter_flag("/Fp"));
        assert!(should_filter_flag("/FpDebug/test.pch"));
        assert!(should_filter_flag("/Fp\"C:\\path\\file.pch\""));

        // Should NOT filter floating-point model flags (they have colons!)
        assert!(!should_filter_flag("/fp:precise"));
        assert!(!should_filter_flag("/fp:fast"));
        assert!(!should_filter_flag("/fp:strict"));
        assert!(!should_filter_flag("/Fp:precise")); // Alternative valid form
        assert!(!should_filter_flag("/fp:contract"));
        assert!(!should_filter_flag("/fp:except"));
        assert!(!should_filter_flag("/fp:except-"));

        // Should NOT filter force includes
        assert!(!should_filter_flag("/FI"));
        assert!(!should_filter_flag("/FIheader.h"));

        // Case insensitive for PCH
        assert!(should_filter_flag("/yc"));
        assert!(should_filter_flag("/YC"));
        assert!(should_filter_flag("/fp\"test.pch\"")); // PCH (no colon!)

        // Should not filter other flags
        assert!(!should_filter_flag("/c"));
        assert!(!should_filter_flag("/Ox"));
    }

    // ----------------------------------------------------------------------------
    // Tests for normalize_path()
    // ----------------------------------------------------------------------------

    #[test]
    fn test_normalize_path_with_double_backslash() {
        let path = PathBuf::from(r"C:\foo\bar\\baz\file.cpp");
        let normalized = normalize_path(&path);
        assert_eq!(normalized, PathBuf::from(r"C:\foo\bar\baz\file.cpp"));
    }

    #[test]
    fn test_normalize_path_normal() {
        let path = PathBuf::from(r"C:\foo\bar\baz\file.cpp");
        let normalized = normalize_path(&path);
        assert_eq!(normalized, path);
    }

    // ----------------------------------------------------------------------------
    // Tests for path_to_normalized_string()
    // ----------------------------------------------------------------------------

    #[test]
    fn test_path_to_normalized_string() {
        let path = PathBuf::from(r"S:\Acme\Project\src\project\obj\amd64\\bond\core\file.cpp");
        let normalized = path_to_normalized_string(&path);
        // Should not contain double backslashes
        assert!(!normalized.contains(r"\\"));
        // Should contain the components
        assert!(normalized.contains("bond"));
        assert!(normalized.contains("core"));
    }

    // ----------------------------------------------------------------------------
    // Tests for parse_cl_command()
    // ----------------------------------------------------------------------------

    #[test]
    fn test_parse_cl_command_single_file() {
        let project_ctx = ProjectContext {
            project_path: PathBuf::from(r"C:\project\test.vcxproj"),
            project_dir: PathBuf::from(r"C:\project"),
        };

        // Test with UNQUOTED path (like real MSBuild logs)
        let line = r#"  C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Tools\MSVC\14.44.35207\bin\HostX64\x64\CL.exe /c /I"include" main.cpp"#;
        let commands = parse_cl_command(line, &project_ctx, 200).unwrap();

        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].directory, r"C:\project");
        // File should now be absolute
        assert_eq!(commands[0].file, r"C:\project\main.cpp");
        assert!(commands[0]
            .command
            .contains(r#""C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Tools\MSVC\14.44.35207\bin\HostX64\x64\CL.exe""#));
        assert!(commands[0].command.contains(r#"/I"include""#));
        assert!(commands[0].command.contains(r#"C:\project\main.cpp"#));
    }

    #[test]
    fn test_parse_cl_command_multiple_files() {
        let project_ctx = ProjectContext {
            project_path: PathBuf::from(r"C:\project\test.vcxproj"),
            project_dir: PathBuf::from(r"C:\project"),
        };

        // Test with UNQUOTED path (like real MSBuild logs)
        let line = r#"  C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Tools\MSVC\14.44.35207\bin\HostX64\x64\CL.exe /c /Ox main.cpp util.cpp helper.c"#;
        let commands = parse_cl_command(line, &project_ctx, 200).unwrap();

        assert_eq!(commands.len(), 3);
        // Files should now be absolute
        assert_eq!(commands[0].file, r"C:\project\main.cpp");
        assert_eq!(commands[1].file, r"C:\project\util.cpp");
        assert_eq!(commands[2].file, r"C:\project\helper.c");

        // All should have same directory and flags
        for cmd in &commands {
            assert_eq!(cmd.directory, r"C:\project");
            assert!(cmd.command.contains("/Ox"));
            assert!(cmd
                .command
                .contains(r#""C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Tools\MSVC\14.44.35207\bin\HostX64\x64\CL.exe""#));
        }
    }

    #[test]
    fn test_parse_cl_command_filters_pch() {
        let project_ctx = ProjectContext {
            project_path: PathBuf::from(r"C:\project\test.vcxproj"),
            project_dir: PathBuf::from(r"C:\project"),
        };

        // Test with UNQUOTED path (like real MSBuild logs)
        let line = r#"  C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Tools\MSVC\14.44.35207\bin\HostX64\x64\CL.exe /c /YuStdafx.h /FpDebug/test.pch /FIcommon.h main.cpp"#;
        let commands = parse_cl_command(line, &project_ctx, 200).unwrap();

        assert_eq!(commands.len(), 1);

        // Should filter /Yu and /Fp but keep /FI
        assert!(!commands[0].command.contains("/Yu"));
        assert!(!commands[0].command.contains("/Fp"));
        assert!(commands[0].command.contains("/FIcommon.h"));
    }

    #[test]
    fn test_parse_cl_command_preserves_fp_model() {
        let project_ctx = ProjectContext {
            project_path: PathBuf::from(r"C:\project\test.vcxproj"),
            project_dir: PathBuf::from(r"C:\project"),
        };

        // Test that /fp:precise (floating-point model) is preserved while /Fp (PCH) is filtered
        let line = r#"  C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Tools\MSVC\14.44.35207\bin\HostX64\x64\CL.exe /c /fp:precise /YuStdafx.h /Fp"Debug\test.pch" /Od main.cpp"#;
        let commands = parse_cl_command(line, &project_ctx, 200).unwrap();

        assert_eq!(commands.len(), 1);

        // Should keep /fp:precise (floating-point model)
        assert!(
            commands[0].command.contains("/fp:precise"),
            "Command should contain /fp:precise but got: {}",
            commands[0].command
        );

        // Should filter /Yu and /Fp"..." (PCH flags)
        assert!(
            !commands[0].command.contains("/Yu"),
            "Command should not contain /Yu"
        );
        assert!(
            !commands[0].command.contains("/Fp\""),
            "Command should not contain /Fp with quotes"
        );

        // Should keep other flags
        assert!(commands[0].command.contains("/Od"));
    }

    #[test]
    fn test_parse_cl_command_quoted_file() {
        let project_ctx = ProjectContext {
            project_path: PathBuf::from(r"C:\project\test.vcxproj"),
            project_dir: PathBuf::from(r"C:\project"),
        };

        // Test with UNQUOTED path (like real MSBuild logs)
        let line = r#"  C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Tools\MSVC\14.44.35207\bin\HostX64\x64\CL.exe /c "path with spaces\main.cpp""#;
        let commands = parse_cl_command(line, &project_ctx, 200).unwrap();

        assert_eq!(commands.len(), 1);
        // File field should be absolute with no quotes
        assert_eq!(commands[0].file, r"C:\project\path with spaces\main.cpp");
        // Command should have absolute path with quotes
        assert!(
            commands[0]
                .command
                .contains(r#"C:\project\path with spaces\main.cpp"#)
        );
    }

    #[test]
    fn test_parse_cl_command_full_path_with_spaces() {
        let project_ctx = ProjectContext {
            project_path: PathBuf::from(r"C:\project\test.vcxproj"),
            project_dir: PathBuf::from(r"C:\project"),
        };

        // Test with QUOTED CL.exe path (ensure backward compatibility)
        let line = r#"  "C:\Program Files\MSVC\bin\HostX64\x64\CL.exe" /c main.cpp"#;
        let commands = parse_cl_command(line, &project_ctx, 200).unwrap();

        assert_eq!(commands.len(), 1);
        // Should preserve full path with quotes due to spaces
        assert!(
            commands[0]
                .command
                .contains(r#""C:\Program Files\MSVC\bin\HostX64\x64\CL.exe""#)
        );
        assert!(commands[0].command.contains(r"C:\project\main.cpp"));
    }

    #[test]
    fn test_parse_cl_command_unquoted_path_with_spaces() {
        let project_ctx = ProjectContext {
            project_path: PathBuf::from(r"C:\project\test.vcxproj"),
            project_dir: PathBuf::from(r"C:\project"),
        };

        // Test with UNQUOTED CL.exe path with spaces (real MSBuild logs)
        let line = r#"  C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Tools\MSVC\14.44.35207\bin\HostX64\x64\CL.exe /c main.cpp"#;
        let commands = parse_cl_command(line, &project_ctx, 200).unwrap();

        assert_eq!(commands.len(), 1);
        // Should quote the path with spaces
        assert!(commands[0]
            .command
            .contains(r#""C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Tools\MSVC\14.44.35207\bin\HostX64\x64\CL.exe""#));
        assert!(commands[0].command.contains(r"C:\project\main.cpp"));
    }

    // ----------------------------------------------------------------------------
    // Tests for clean_include_path()
    // ----------------------------------------------------------------------------

    #[test]
    fn test_clean_include_path_quoted_with_trailing_backslash() {
        assert_eq!(
            clean_include_path(r#"/I"C:\path\to\dir\\""#),
            r#"/I"C:\path\to\dir""#
        );
    }

    #[test]
    fn test_clean_include_path_quoted_no_trailing() {
        assert_eq!(
            clean_include_path(r#"/I"C:\path\to\dir""#),
            r#"/I"C:\path\to\dir""#
        );
    }

    #[test]
    fn test_clean_include_path_unquoted_with_trailing() {
        assert_eq!(
            clean_include_path(r#"/IC:\path\to\dir\"#),
            r#"/IC:\path\to\dir"#
        );
    }

    #[test]
    fn test_clean_include_path_multiple_trailing() {
        assert_eq!(
            clean_include_path(r#"/I"C:\path\to\dir\\\\\""#),
            r#"/I"C:\path\to\dir""#
        );
    }

    #[test]
    fn test_clean_include_path_not_include_flag() {
        assert_eq!(clean_include_path(r#"/Od"#), r#"/Od"#);
    }

    #[test]
    fn test_clean_include_path_root_drive() {
        // Root path like C:\ should be preserved
        assert_eq!(clean_include_path(r#"/I"C:\""#), r#"/I"C:\""#);
    }

    #[test]
    fn test_clean_include_path_lowercase_flag() {
        // Should work with lowercase /i as well
        assert_eq!(
            clean_include_path(r#"/i"C:\path\to\dir\\""#),
            r#"/i"C:\path\to\dir""#
        );
    }

    #[test]
    fn test_clean_include_path_real_world_example() {
        // Real example from the XStore build
        assert_eq!(
            clean_include_path(
                r#"/I"S:\Azure\Storage-XStore\src\XAggregator\XsdMacroGen\obj\amd64\\""#
            ),
            r#"/I"S:\Azure\Storage-XStore\src\XAggregator\XsdMacroGen\obj\amd64""#
        );
    }

    // ----------------------------------------------------------------------------
    // Tests for resolve_source_file_path()
    // ----------------------------------------------------------------------------

    #[test]
    fn test_resolve_source_file_path_relative() {
        let working_dir = PathBuf::from(r"C:\project");
        let source = "src\\main.cpp";
        let resolved = resolve_source_file_path(source, &working_dir);
        assert_eq!(resolved, PathBuf::from(r"C:\project\src\main.cpp"));
    }

    #[test]
    fn test_resolve_source_file_path_parent_directory() {
        let working_dir = PathBuf::from(r"C:\project\SubDir");
        let source = r"..\Common\shared.cpp";
        let resolved = resolve_source_file_path(source, &working_dir);
        assert_eq!(
            resolved,
            PathBuf::from(r"C:\project\SubDir\..\Common\shared.cpp")
        );
    }

    #[test]
    fn test_resolve_source_file_path_already_absolute() {
        let working_dir = PathBuf::from(r"C:\project");
        let source = r"D:\external\library\file.cpp";
        let resolved = resolve_source_file_path(source, &working_dir);
        assert_eq!(resolved, PathBuf::from(r"D:\external\library\file.cpp"));
    }

    #[test]
    fn test_resolve_source_file_path_quoted() {
        let working_dir = PathBuf::from(r"C:\project");
        let source = r#""src\main.cpp""#;
        let resolved = resolve_source_file_path(source, &working_dir);
        assert_eq!(resolved, PathBuf::from(r"C:\project\src\main.cpp"));
    }

    #[test]
    fn test_resolve_source_file_path_current_directory() {
        let working_dir = PathBuf::from(r"C:\project");
        let source = r".\main.cpp";
        let resolved = resolve_source_file_path(source, &working_dir);
        assert_eq!(resolved, PathBuf::from(r"C:\project\.\main.cpp"));
    }

    // ----------------------------------------------------------------------------
    // Tests for normalize_path()
    // ----------------------------------------------------------------------------

    #[test]
    fn test_normalize_path_triple_backslash() {
        let path = PathBuf::from(r"C:\foo\bar\\\baz\file.cpp");
        let normalized = normalize_path(&path);
        // Should eliminate all redundant backslashes
        assert_eq!(normalized, PathBuf::from(r"C:\foo\bar\baz\file.cpp"));
    }

    #[test]
    fn test_normalize_path_mixed_separators() {
        // On Windows, PathBuf handles / and \ differently depending on the input
        let path = PathBuf::from(r"C:\foo/bar\baz/file.cpp");
        let normalized = normalize_path(&path);
        // The path has 5 meaningful components, but the mixed separator might create more
        // Just verify normalization happened
        let normalized_str = normalized.display().to_string();
        assert!(normalized_str.contains("foo"));
        assert!(normalized_str.contains("bar"));
        assert!(normalized_str.contains("baz"));
        assert!(normalized_str.contains("file.cpp"));
    }

    // ----------------------------------------------------------------------------
    // Tests for tokenize_command_line()
    // ----------------------------------------------------------------------------

    #[test]
    fn test_tokenize_empty_string() {
        let tokens = tokenize_command_line("");
        assert_eq!(tokens.len(), 0);
    }

    #[test]
    fn test_tokenize_only_whitespace() {
        let tokens = tokenize_command_line("   \t  ");
        assert_eq!(tokens.len(), 0);
    }

    #[test]
    fn test_tokenize_unclosed_quote() {
        // Unclosed quote - should still tokenize (quote becomes part of token)
        let tokens = tokenize_command_line(r#"cl.exe /I"C:\Program Files"#);
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0], "cl.exe");
        assert_eq!(tokens[1], r#"/I"C:\Program Files"#);
    }

    #[test]
    fn test_tokenize_adjacent_quotes() {
        let tokens = tokenize_command_line(r#"cl.exe ""file.cpp"""#);
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0], "cl.exe");
        assert_eq!(tokens[1], r#"""file.cpp"""#);
    }

    #[test]
    fn test_tokenize_tabs() {
        let tokens = tokenize_command_line("cl.exe\t/c\tmain.cpp");
        assert_eq!(tokens, vec!["cl.exe", "/c", "main.cpp"]);
    }

    // ----------------------------------------------------------------------------
    // Tests for is_source_file()
    // ----------------------------------------------------------------------------

    #[test]
    fn test_is_source_file_uppercase_extensions() {
        assert!(is_source_file("MAIN.CPP"));
        assert!(is_source_file("FILE.C"));
        assert!(is_source_file("CODE.CXX"));
        assert!(is_source_file("TEST.CC"));
    }

    #[test]
    fn test_is_source_file_mixed_case_extensions() {
        assert!(is_source_file("main.CpP"));
        assert!(is_source_file("file.Cpp"));
    }

    #[test]
    fn test_is_source_file_quoted_paths() {
        assert!(is_source_file(r#""path\to\file.cpp""#));
        assert!(is_source_file(r#""test.c""#));
    }

    #[test]
    fn test_is_source_file_with_path() {
        assert!(is_source_file(r"C:\project\src\main.cpp"));
        assert!(is_source_file(r"relative\path\file.c"));
    }

    #[test]
    fn test_is_source_file_not_source() {
        assert!(!is_source_file("header.h"));
        assert!(!is_source_file("library.lib"));
        assert!(!is_source_file("object.obj"));
        assert!(!is_source_file("executable.exe"));
        assert!(!is_source_file("archive.a"));
        assert!(!is_source_file("README.md"));
    }

    // ----------------------------------------------------------------------------
    // Tests for handler functions
    // ----------------------------------------------------------------------------

    #[test]
    fn test_handle_node_prefix_valid() {
        let mut state = ProcessingState::new();
        let pattern = node_prefix_pattern().unwrap();

        handle_node_prefix("  7>Project ...", &pattern, &mut state, 100);

        assert_eq!(state.current_prefix, Some(7));
    }

    #[test]
    fn test_handle_node_prefix_no_match() {
        let mut state = ProcessingState::new();
        let pattern = node_prefix_pattern().unwrap();

        handle_node_prefix("Project without prefix", &pattern, &mut state, 100);

        assert_eq!(state.current_prefix, None);
    }

    #[test]
    fn test_handle_project_on_node_parallel_build() {
        let mut state = ProcessingState::new();
        let pattern = project_on_node_pattern().unwrap();
        let line = r#"4>Project "C:\path\to\project.vcxproj" on node 3 (Build target(s))."#;

        let result = handle_project_on_node(line, &pattern, &mut state, 100);

        assert!(result.is_ok());
        assert_eq!(state.prefix_to_project.len(), 1);
        assert!(state.prefix_to_project.contains_key(&4));
        assert!(state.current_project.is_some());
        assert_eq!(
            state.prefix_to_project.get(&4).unwrap().project_path,
            PathBuf::from(r"C:\path\to\project.vcxproj")
        );
    }

    #[test]
    fn test_handle_nested_project_pattern() {
        let mut state = ProcessingState::new();
        let pattern = nested_project_pattern().unwrap();
        let line = r#"    44>Project "S:\Acme\corp\src\foo\baz.proj" (44) is building "S:\Acme\corp\src\foo\bar.vcxproj" (54) on node 13 (default targets)."#;

        let result = handle_nested_project(line, &pattern, &mut state, 100);

        assert!(result.is_ok());
        assert_eq!(state.prefix_to_project.len(), 1);
        assert!(state.prefix_to_project.contains_key(&54));
        assert_eq!(
            state.prefix_to_project.get(&54).unwrap().project_path,
            PathBuf::from(r"S:\Acme\corp\src\foo\bar.vcxproj")
        );
    }

    #[test]
    fn test_handle_from_project_sequential_build() {
        let mut state = ProcessingState::new();
        let pattern = from_project_pattern().unwrap();
        let line = r#"Target "ClCompile" from project "C:\path\to\project.vcxproj""#;

        handle_from_project(line, &pattern, &mut state, 100);

        assert!(state.current_project.is_some());
        assert_eq!(
            state.current_project.as_ref().unwrap().project_path,
            PathBuf::from(r"C:\path\to\project.vcxproj")
        );
    }

    #[test]
    fn test_handle_cl_command_with_context() {
        let mut state = ProcessingState::new();
        state.current_project = Some(ProjectContext {
            project_path: PathBuf::from(r"C:\project\test.vcxproj"),
            project_dir: PathBuf::from(r"C:\project"),
        });

        let pattern = compile_command_pattern().unwrap();
        let line = r#"  C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Tools\MSVC\14.44.35207\bin\HostX64\x64\CL.exe /c main.cpp"#;

        let result = handle_cl_command(line, &pattern, &state, 100);

        assert!(result.is_ok());
        let commands = result.unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].file, r"C:\project\main.cpp");
    }

    #[test]
    fn test_handle_cl_command_no_context() {
        let state = ProcessingState::new();
        let pattern = compile_command_pattern().unwrap();
        let line = r#"  CL.exe /c main.cpp"#;

        let result = handle_cl_command(line, &pattern, &state, 100);

        assert!(result.is_ok());
        let commands = result.unwrap();
        assert_eq!(commands.len(), 0); // No commands when no context
    }

    #[test]
    fn test_handle_cl_command_not_cl_command() {
        let state = ProcessingState::new();
        let pattern = compile_command_pattern().unwrap();
        let line = r#"This is not a CL.exe command"#;

        let result = handle_cl_command(line, &pattern, &state, 100);

        assert!(result.is_ok());
        let commands = result.unwrap();
        assert_eq!(commands.len(), 0);
    }

    #[test]
    fn test_processing_state_get_active_project_with_prefix() {
        let mut state = ProcessingState::new();
        let ctx = ProjectContext {
            project_path: PathBuf::from(r"C:\prefix\project.vcxproj"),
            project_dir: PathBuf::from(r"C:\prefix"),
        };
        state.prefix_to_project.insert(7, ctx.clone());
        state.current_prefix = Some(7);

        let active = state.get_active_project();

        assert!(active.is_some());
        assert_eq!(
            active.unwrap().project_path,
            PathBuf::from(r"C:\prefix\project.vcxproj")
        );
    }

    #[test]
    fn test_processing_state_get_active_project_fallback() {
        let mut state = ProcessingState::new();
        state.current_project = Some(ProjectContext {
            project_path: PathBuf::from(r"C:\fallback\project.vcxproj"),
            project_dir: PathBuf::from(r"C:\fallback"),
        });
        state.current_prefix = Some(99); // Prefix not in map

        let active = state.get_active_project();

        assert!(active.is_some());
        assert_eq!(
            active.unwrap().project_path,
            PathBuf::from(r"C:\fallback\project.vcxproj")
        );
    }

    #[test]
    fn test_processing_state_get_active_project_no_prefix() {
        let mut state = ProcessingState::new();
        state.current_project = Some(ProjectContext {
            project_path: PathBuf::from(r"C:\sequential\project.vcxproj"),
            project_dir: PathBuf::from(r"C:\sequential"),
        });

        let active = state.get_active_project();

        assert!(active.is_some());
        assert_eq!(
            active.unwrap().project_path,
            PathBuf::from(r"C:\sequential\project.vcxproj")
        );
    }

    // ----------------------------------------------------------------------------
    // Tests for progress bar setup functions
    // ----------------------------------------------------------------------------

    #[test]
    fn test_setup_read_progress_bar_enabled() {
        let multi = MultiProgress::new();
        let pb = setup_read_progress_bar(true, 1000, &multi).unwrap();
        // Should create a visible progress bar (not hidden)
        // We can't directly test visibility, but we can verify it doesn't panic
        pb.finish_and_clear();
    }

    #[test]
    fn test_setup_read_progress_bar_disabled() {
        let multi = MultiProgress::new();
        let pb = setup_read_progress_bar(false, 1000, &multi).unwrap();
        // Should create a hidden progress bar
        pb.finish_and_clear();
    }

    #[test]
    fn test_setup_write_progress_bar_enabled() {
        let multi = MultiProgress::new();
        let pb = setup_write_progress_bar(true, &multi).unwrap();
        // Should create a visible spinner
        pb.finish_and_clear();
    }

    #[test]
    fn test_setup_write_progress_bar_disabled() {
        let multi = MultiProgress::new();
        let pb = setup_write_progress_bar(false, &multi).unwrap();
        // Should create a hidden progress bar
        pb.finish_and_clear();
    }
}
