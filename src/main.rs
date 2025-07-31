use clap::Parser;
use crossbeam_channel::{Receiver, Sender, unbounded};
use dashmap::DashMap;
use ms2cc::CompileCommand;
use serde_json::{to_writer, to_writer_pretty};
use std::fs::{File, read_dir};
use std::io::{BufRead, BufReader, BufWriter};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use std::{process, thread};

// Configuration constants
const DEFAULT_BUFFER_SIZE: usize = 64 * 1024; // 64KB buffer for file I/O
const RECV_TIMEOUT_MS: u64 = 500; // Timeout for channel receive operations
const MULTILINE_RESERVE_SIZE: usize = 512; // Pre-allocation for multi-line commands
const TOKEN_CAPACITY_DIVISOR: usize = 8; // Rough estimate for token capacity
const DEFAULT_MAX_THREADS: u8 = 8; // Default number of threads per task
const EXIT_FAILURE: i32 = -1; // Exit code for failure
const HEADER_WIDTH: usize = 50; // Width for centered header text

/// Command line arguments
#[derive(Parser)]
#[command(
    version,
    about = "Tool to generate a compile_commands.json database from an msbuild.log file."
)]
struct Cli {
    /// Path to msbuild.log
    #[arg(short('i'), long)]
    input_file: PathBuf,

    /// Output JSON file
    #[arg(short('o'), long, default_value = "compile_commands.json")]
    output_file: PathBuf,

    /// Pretty print output JSON file
    #[arg(short('p'), long, default_value_t = false)]
    pretty_print: bool,

    /// Path to source code
    #[arg(short('d'), long)]
    source_directory: PathBuf,

    /// Directories to exclude during traversal (comma-separated)
    #[arg(short('x'), long, value_delimiter = ',', default_values_t = [".git".to_string()])]
    exclude_directories: Vec<String>,

    /// File extensions to process (comma-separated)
    #[arg(short('e'), long, value_delimiter = ',', default_values_t = [
        "c".to_string(),
        "cc".to_string(),
        "cpp".to_string(),
        "cxx".to_string(),
        "c++".to_string(),
        "h".to_string(),
        "hh".to_string(),
        "hpp".to_string(),
        "hxx".to_string(),
        "h++".to_string(),
        "inl".to_string()]
    )]
    file_extensions: Vec<String>,

    /// Name of compiler executable
    #[arg(short('c'), long, name = "EXE", default_value = "cl.exe")]
    compiler_executable: String,

    /// Max number of threads per task
    #[arg(short('t'), long, default_value_t = DEFAULT_MAX_THREADS)]
    max_threads: u8,
}

/// Error handler.  Reports any received errors to `STDERR`.
fn error_handler(error_rx: Receiver<String>) {
    while let Ok(e) = error_rx.recv() {
        eprintln!("{e}");
    }
}

/// Explores the directory tree `path`, visiting all directories, and sending
/// any files found on the `entry_tx` sender channel. Any IO errors are reported
/// to the `error_tx` channel.
fn find_all_files(
    directory_rx: Receiver<PathBuf>,
    directory_tx: Sender<PathBuf>,
    entry_tx: Sender<PathBuf>,
    error_tx: Sender<String>,
    exclude_directories: &[String],
    file_extensions: &[String],
) {
    while let Ok(path) = directory_rx
        .recv_timeout(std::time::Duration::from_millis(RECV_TIMEOUT_MS))
    {
        let reader = match read_dir(&path) {
            Ok(r) => r,
            Err(e) => {
                let e = format!("read_dir error for {path:?}: {e}");
                let _ = error_tx.send(e);
                continue;
            }
        };
        for entry in reader {
            let entry = match entry {
                Ok(de) => de,
                Err(e) => {
                    let e = format!("Failed to read from {path:?}: {e}",);
                    let _ = error_tx.send(e);
                    continue;
                }
            };

            let path = entry.path();
            if path.is_dir() {
                // Skip directories specified in exclude list
                if let Some(dir_name) =
                    path.file_name().and_then(|n| n.to_str())
                {
                    let dir_name_lower = dir_name.to_lowercase();
                    if exclude_directories
                        .iter()
                        .any(|exclude| exclude.to_lowercase() == dir_name_lower)
                    {
                        continue;
                    }
                }
                let _ = directory_tx.send(path);
                continue;
            }

            if path.is_file() {
                // Only process C/C++ source and header files
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    let ext_lower = ext.to_lowercase();
                    if file_extensions.iter().any(|allowed_ext| {
                        allowed_ext.to_lowercase() == ext_lower
                    }) {
                        // Normalize the path
                        if let Some(path) = path
                            .to_str()
                            .map(|s| s.to_lowercase())
                            .map(PathBuf::from)
                        {
                            let _ = entry_tx.send(path);
                        } else {
                            let e = format!("Failed to normalize {path:?}");
                            let _ = error_tx.send(e);
                        }
                    }
                }
                continue;
            }

            let e = format!("Unknown entry {path:?}");
            let _ = error_tx.send(e);
        }
    }
}

/// Generates a hash map of file/path entries from all files sent to the
/// `entry_rx` channel.  This map is used for compile command entries found in
/// `msbuild.log` that only include a file name without a path.
///
/// NOTE: All paths are expected to be lowercase.
fn build_file_map(
    entry_rx: Receiver<PathBuf>,
    tree: Arc<DashMap<PathBuf, PathBuf>>,
) {
    // Generate a map of files and their directories
    while let Ok(path) =
        entry_rx.recv_timeout(std::time::Duration::from_millis(RECV_TIMEOUT_MS))
    {
        // Files are already filtered to relevant extensions
        if let (Some(file_name), Some(parent)) =
            (path.file_name(), path.parent())
        {
            let file_name = PathBuf::from(file_name);
            let parent = PathBuf::from(parent);

            // Add KV pair (file/path) to the hash table; clear on collision
            tree.entry(file_name)
                .and_modify(|absolute_path: &mut PathBuf| absolute_path.clear())
                .or_insert(parent);
        }
    }
}

/// Helper function to check if a line ends with a C/C++ source file extension
/// (possibly followed by quotes, spaces, or other whitespace)
fn ends_with_cpp_source_file(line: &str, file_extensions: &[String]) -> bool {
    let line = line.trim_end(); // Remove trailing whitespace
    let line = line.trim_end_matches(['"', '\'']); // Remove trailing quotes

    // Check for C/C++ source file extensions
    file_extensions
        .iter()
        .any(|ext| line.to_lowercase().ends_with(&ext.to_lowercase()))
}

/// Searches an `msbuild.log` for all lines containing `s` string and sends
/// them out on the `tx` channel. Any errors are reported on the `e_tx` channel.
fn find_all_lines(
    reader: BufReader<File>,
    s: &str,
    tx: Sender<String>,
    e_tx: Sender<String>,
    file_extensions: &[String],
) {
    // Pre-lowercase the compiler executable for comparison
    let compiler_exe_lower = s.to_lowercase();

    let mut compile_command = String::new();
    let mut multi_line = false;
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(e) => {
                let e = format!("Skipping a read error in log file: {e}");
                let _ = e_tx.send(e);
                continue;
            }
        };

        // Convert to lowercase for simplified pattern matching
        let lowercase = line.to_lowercase();

        // Check our state
        if !multi_line {
            // Skip non compile command lines
            if !lowercase.contains(&compiler_exe_lower) {
                continue;
            }

            // Is this a complete compile command (cl.exe ... file.cpp)?
            if ends_with_cpp_source_file(&lowercase, file_extensions) {
                let _ = tx.send(line);
                continue;
            }

            // This compile command is on multiple lines (cl.exe ...)
            multi_line = true;
            compile_command = line;
            // Pre-allocate space for multi-line commands to reduce reallocations
            compile_command.reserve(MULTILINE_RESERVE_SIZE);
            continue;
        } else {
            // Append to the previous line with space separator
            compile_command.push(' ');
            compile_command.push_str(&line);

            // Is this the end of the command (... file.cpp)?
            if ends_with_cpp_source_file(&lowercase, file_extensions) {
                let _ = tx.send(compile_command);

                // Reset state
                compile_command = String::new();
                multi_line = false;
                continue;
            }

            // This should be part of the line (... /Zi /EHsc ...), but let's
            // make sure.
            if lowercase.contains(&compiler_exe_lower) {
                // We encountered a new line containing cl.exe before reaching
                // completing the previous compile command.

                // We'll log an error, reset the state and continue.
                let e = format!(
                    "Unexpected line {} while building the compile command {}",
                    line, compile_command
                );
                let _ = e_tx.send(e);

                // Reset state
                compile_command = String::new();
                multi_line = false;
            }
        }
    }
}

/// Listens on the `rx` channel for strings and strips them of all superfluous
/// characters.  Sends the updated string on the `tx` channel.
fn cleanup_line(rx: Receiver<String>, tx: Sender<String>) {
    while let Ok(mut s) = rx.recv() {
        // Only process if quotes are present to avoid unnecessary work
        if s.contains('"') {
            s.retain(|c| c != '"');
        }
        let _ = tx.send(s);
    }
}

/// Converts strings received on the `rx` channel into tokens and sends them out
/// on the `tx` channel.
fn tokenize_lines(rx: Receiver<String>, tx: Sender<Vec<String>>) {
    while let Ok(s) = rx.recv() {
        // Pre-allocate with estimated capacity to reduce reallocations
        let mut tokens = Vec::with_capacity(s.len() / TOKEN_CAPACITY_DIVISOR); // Rough estimate
        tokens.extend(s.split_whitespace().map(String::from));
        let _ = tx.send(tokens);
    }
}

/// When a compile command in the log file uses an absolute path to the source file, all required
/// components exist to generate a `CompileCommand`.
fn create_compile_command(
    path: PathBuf,
    arguments: Vec<String>,
    error_tx: Sender<String>,
) -> Option<CompileCommand> {
    let directory = match path.parent() {
        Some(parent) => PathBuf::from(parent),
        None => {
            let e = format!("Missing parent component in {:?}", path);
            let _ = error_tx.send(e);
            return None;
        }
    };

    let file = match path.file_name() {
        Some(file_name) => PathBuf::from(file_name),
        None => {
            let e = format!("Missing file_name component in {:?}", path);
            let _ = error_tx.send(e);
            return None;
        }
    };

    Some(CompileCommand {
        file,
        directory,
        arguments,
    })
}

/// Converts a stream of tokens received on the `rx` channel into a
/// `CompileCommand` and sends it out on the `tx` channel. The `map` generated
/// by `build_file_map` is used to find the paths to any source files that did
/// not include it in `msbuild.log`. Errors are reported on the `error_tx`
/// channel
fn create_compile_commands(
    map: Arc<DashMap<PathBuf, PathBuf>>,
    rx: Receiver<Vec<String>>,
    tx: Sender<CompileCommand>,
    error_tx: Sender<String>,
) {
    while let Ok(arguments) = rx.recv() {
        // The file name should be the last compiler argument
        let arg_path = match arguments.last() {
            Some(path) => path,
            None => {
                let e = String::from("Token vector is empty!");
                let _ = error_tx.send(e);
                continue;
            }
        };

        // Convert to PathBuf and lowercase for processing
        let arg_path_buf = PathBuf::from(arg_path.to_lowercase());

        // Is the last argument in the compile command a file?
        let file_name = match arg_path_buf.file_name() {
            Some(file_name) => PathBuf::from(file_name),
            None => {
                let e = format!("Missing file_name component in {arg_path:?}");
                let _ = error_tx.send(e);
                continue;
            }
        };

        // Does it have an extension?
        if file_name.extension().is_none() {
            let e =
                format!("File name component missing extension {arg_path:?}");
            let _ = error_tx.send(e);
            continue;
        }

        // If we only have a file name or relative path, try to reconstruct an
        // absolute path.

        // First we check our directory tree
        let mut path = PathBuf::new();
        if !arg_path_buf.is_absolute() {
            if let Some(parent) = map.get(&file_name) {
                path = parent.clone();
                path.push(&file_name);
            };
        } else {
            path = arg_path_buf.clone();
        }

        // Last option is trying to reconstruct the path using the /Fo argument.
        if !path.is_absolute() {
            const ARGUMENT: &str = "/Fo";
            if let Some(fo_argument) =
                arguments.iter().find(|s| s.starts_with(ARGUMENT))
            {
                path = PathBuf::from(
                    fo_argument.strip_prefix(ARGUMENT).unwrap().to_lowercase(),
                );

                while path.has_root() {
                    // Test using /Fo path and the filename from the argument.
                    let mut test_path = path.clone();
                    test_path.push(&file_name);

                    // Did we find the path?
                    if test_path.is_file() {
                        path = test_path;
                        break;
                    }

                    // Let's try with the relative path in the argument.
                    test_path.pop();
                    test_path.push(&arg_path_buf);

                    // Did we find the path?
                    if test_path.is_file() {
                        path = test_path;
                        break;
                    }

                    path.pop();

                    // Reached the end?
                    if !path.pop() {
                        break;
                    }
                }
            } else {
                let e =
                    format!("No {ARGUMENT} argument found in {arguments:?}");
                let _ = error_tx.send(e);
                continue;
            }
        }

        if !path.is_absolute() || !path.is_file() {
            let e =
                format!("Failed to retreive an absolute path to {file_name:?}");
            let _ = error_tx.send(e);
            continue;
        }

        // Found the path
        if let Some(cc) =
            create_compile_command(path, arguments, error_tx.clone())
        {
            let _ = tx.send(cc);
        }
    }
}

/// Prints an error message to standard error and exits.
fn exit_with_message(msg: String) -> ! {
    eprintln!("{msg}");
    process::exit(EXIT_FAILURE);
}

fn main() {
    //
    // Input validation
    //

    // Parse command line arguments
    let cli = Cli::parse();

    let package_name = env!("CARGO_PKG_NAME");
    let package_version = env!("CARGO_PKG_VERSION");

    // File reader with larger buffer for better performance
    let input_file_handle = match File::open(&cli.input_file) {
        Ok(handle) => BufReader::with_capacity(DEFAULT_BUFFER_SIZE, handle), // 64KB buffer
        Err(e) => exit_with_message(format!(
            "Failed to open {:?}: {}",
            cli.input_file, e
        )),
    };

    // Early validation: check if input file is empty
    if let Ok(metadata) = std::fs::metadata(&cli.input_file) {
        if metadata.len() == 0 {
            exit_with_message(format!(
                "Input file {:?} is empty",
                cli.input_file
            ));
        }
    }

    // Verify source directory is a valid path
    if !cli.source_directory.is_dir() {
        exit_with_message(format!(
            "Provided path is not a directory: {:?}",
            cli.source_directory
        ));
    }

    // Quick check if source directory is likely to be empty or contain relevant files
    if let Ok(mut entries) = std::fs::read_dir(&cli.source_directory) {
        if entries.next().is_none() {
            exit_with_message(format!(
                "Source directory {:?} appears to be empty",
                cli.source_directory
            ));
        }
    }

    // File writer with buffer for better performance
    let output_file_handle = match File::options()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&cli.output_file)
    {
        Ok(handle) => BufWriter::with_capacity(DEFAULT_BUFFER_SIZE, handle), // 64KB buffer
        Err(e) => exit_with_message(format!(
            "Failed to open {:?}: {}",
            cli.output_file, e
        )),
    };

    //
    // Ready
    //
    println!("==================================================");
    println!(
        "{:^width$}",
        format!("{package_name} v{package_version} - Run Start"),
        width = HEADER_WIDTH
    );
    println!("==================================================");

    let start_time = Instant::now();
    //
    // Step 1
    //
    println!();
    println!("--------------------------------------------------");
    println!(
        "{:^width$}",
        "Generating the lookup tree",
        width = HEADER_WIDTH
    );
    println!("--------------------------------------------------");
    println!();
    println!("Source directory: {:?}", cli.source_directory);
    println!();
    println!("Starting threads:");
    println!(" - 1 error handling thread");
    println!(" - {} directory traversal threads", cli.max_threads);
    println!(" - {} file processing threads", cli.max_threads);
    println!();

    let task_start_time = Instant::now();
    let tree = Arc::new(DashMap::new());
    let (directory_tx, directory_rx) = unbounded();
    let (entry_tx, entry_rx) = unbounded();
    let (error_tx, error_rx) = unbounded();

    // Separate thread for error handling.
    thread::spawn(move || {
        error_handler(error_rx);
    });

    // Traverse the directory tree
    let _ = directory_tx.send(cli.source_directory);
    let mut directory_handles = Vec::new();
    for _ in 0..cli.max_threads {
        let directory_rx = directory_rx.clone();
        let directory_tx = directory_tx.clone();
        let error_tx = error_tx.clone();
        let entry_tx = entry_tx.clone();
        let exclude_directories = cli.exclude_directories.clone();
        let file_extensions = cli.file_extensions.clone();

        let handle = thread::spawn(move || {
            find_all_files(
                directory_rx,
                directory_tx,
                entry_tx,
                error_tx,
                &exclude_directories,
                &file_extensions,
            );
        });

        directory_handles.push(handle);
    }

    // Process discovered files
    let mut entry_handles = Vec::new();
    for _ in 0..cli.max_threads {
        let entry_rx = entry_rx.clone();
        let tree = Arc::clone(&tree);

        let handle = thread::spawn(move || {
            build_file_map(entry_rx, tree);
        });

        entry_handles.push(handle);
    }

    // Close the original sender channels to signal no more work
    drop(directory_tx);
    drop(entry_tx);
    drop(error_tx);

    for handle in directory_handles {
        let _ = handle.join();
    }

    for handle in entry_handles {
        let _ = handle.join();
    }

    let elapsed_time = task_start_time.elapsed();
    println!(
        "Lookup tree generation completed in {:0.02} seconds",
        elapsed_time.as_secs_f32()
    );
    println!();

    //
    // Step 2
    //
    println!("--------------------------------------------------");
    println!(
        "{:^width$}",
        "Generating the database",
        width = HEADER_WIDTH
    );
    println!("--------------------------------------------------");
    println!();
    println!("Input file: {:?}", cli.input_file);
    println!();
    println!("Starting threads:");
    println!(" - 1 error handling thread");
    println!(" - 1 log searching thread");
    println!(" - 1 log entry cleanup thread");
    println!(" - 1 tokenization thread");
    println!(" - 1 compile command generation thread");
    println!();

    let task_start_time = Instant::now();
    let (source_tx, source_rx) = unbounded();
    let (preprocess_tx, preprocess_rx) = unbounded();
    let (token_tx, token_rx) = unbounded();
    let (compile_command_tx, compile_command_rx) = unbounded();
    let (error_tx, error_rx) = unbounded();

    // Separate thread for error handling.
    thread::spawn(move || {
        error_handler(error_rx);
    });

    // Collect all the compile commands from the input file
    let e_tx = error_tx.clone();
    let file_extensions = cli.file_extensions.clone();
    thread::spawn(move || {
        find_all_lines(
            input_file_handle,
            &cli.compiler_executable,
            source_tx,
            e_tx,
            &file_extensions,
        );
    });

    // Remove nested quotes (")
    thread::spawn(move || {
        cleanup_line(source_rx, preprocess_tx);
    });

    // Tokenize
    thread::spawn(move || {
        tokenize_lines(preprocess_rx, token_tx);
    });

    // Verify the input
    let e_tx = error_tx.clone();
    thread::spawn(move || {
        create_compile_commands(tree, token_rx, compile_command_tx, e_tx);
    });

    // Generate the compile_commands.json file
    let compile_commands: Vec<_> = compile_command_rx.iter().collect();

    // Early exit if no compile commands found
    if compile_commands.is_empty() {
        println!("Warning: No compile commands found in the log file");
    }
    let elapsed_time = task_start_time.elapsed();
    println!(
        "Database generation completed in {:0.02} seconds",
        elapsed_time.as_secs_f32()
    );
    println!();

    //
    // Step 3
    //
    println!("--------------------------------------------------");
    println!(
        "{:^width$}",
        "Writing the database to disk",
        width = HEADER_WIDTH
    );
    println!("--------------------------------------------------");
    println!();
    println!("Output file: {:?}", cli.output_file);
    println!();

    let task_start_time = Instant::now();

    let writer_function = if cli.pretty_print {
        to_writer_pretty
    } else {
        to_writer
    };

    if let Err(e) = writer_function(output_file_handle, &compile_commands) {
        let m = format!("Failed to write {:?}: {}", cli.output_file, e);
        exit_with_message(m);
    }
    let elapsed_time = task_start_time.elapsed();
    println!(
        "Database written in {:0.02} seconds",
        elapsed_time.as_secs_f32()
    );
    println!();

    //
    // Finished
    //
    let elapsed_time = start_time.elapsed();
    println!("==================================================");
    println!("{:^width$}", "Run completed", width = HEADER_WIDTH);
    println!("==================================================");
    println!();
    println!("Total entries written: {:}", compile_commands.len());
    println!("Output location: {:?}", cli.output_file);
    println!(
        "Total time elapsed: {:0.02} seconds",
        elapsed_time.as_secs_f32()
    );
    println!();
    println!("==================================================");
}
