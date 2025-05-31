use clap::Parser;
use crossbeam_channel::{Receiver, Sender, unbounded};
use dashmap::DashMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs::{File, read_dir};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use std::{process, thread};

/// compile_commands.json entry descriptor
#[derive(Deserialize, Serialize)]
struct CompileCommand {
    file: PathBuf,
    directory: PathBuf,
    arguments: Vec<String>,
}

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

    /// Path to source code
    #[arg(short('d'), long)]
    source_directory: PathBuf,

    /// Name of compiler executable
    #[arg(short('c'), long, name = "EXE", default_value = "cl.exe")]
    compiler_executable: String,

    /// Max number of threads per task
    #[arg(short('t'), long, default_value_t = 8)]
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
) {
    while let Ok(path) =
        directory_rx.recv_timeout(std::time::Duration::from_millis(500))
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
                let _ = directory_tx.send(path);
                continue;
            }

            if path.is_file() {
                // Normalize the path
                if let Some(path) =
                    path.to_str().map(|s| s.to_lowercase()).map(PathBuf::from)
                {
                    let _ = entry_tx.send(path);
                } else {
                    let e = format!("Failed to normalize {path:?}");
                    let _ = error_tx.send(e);
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
        entry_rx.recv_timeout(std::time::Duration::from_millis(500))
    {
        // Test if entry is a file with an extension
        if path.extension().is_some() {
            let file_name = PathBuf::from(path.file_name().unwrap());
            let parent = PathBuf::from(path.parent().unwrap());

            // Add KV pair (file/path) to the hash table; clear on collision
            tree.entry(file_name)
                .and_modify(|absolute_path: &mut PathBuf| absolute_path.clear())
                .or_insert(parent);
        }
    }
}

/// Searches an `msbuild.log` for all lines containing `s` string and sends
/// them out on the `tx` channel. Any errors are reported on the `e_tx` channel.
fn find_all_lines(
    reader: BufReader<File>,
    s: &str,
    tx: Sender<String>,
    e_tx: Sender<String>,
) {
    const PATTERN: &str = r#"(.c|.cc|.cpp|.cxx)"?\s*$"#;
    let re = match Regex::new(PATTERN) {
        Ok(re) => re,
        Err(e) => {
            let e = format!("Error creating regular expression {PATTERN}: {e}");
            let _ = e_tx.send(e);
            return;
        }
    };

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
            if !lowercase.contains(s) {
                continue;
            }

            // Is this a complete compile command (cl.exe ... file.cpp)?
            if re.is_match(&lowercase) {
                let _ = tx.send(line);
                continue;
            }

            // This compile command is on multiple lines (cl.exe ...)
            multi_line = true;
            compile_command = line;
            continue;
        } else {
            // Append to the previous line
            compile_command.push_str(&line);

            // Is this the end of the command (... file.cpp)?
            if re.is_match(&lowercase) {
                let _ = tx.send(compile_command);

                // Reset state
                compile_command = String::new();
                multi_line = false;
                continue;
            }

            // This should be part of the line (... /Zi /EHsc ...), but let's
            // make sure.
            if lowercase.contains(s) {
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
    while let Ok(s) = rx.recv() {
        let s = s.replace("\"", "");
        let _ = tx.send(s);
    }
}

/// Converts strings received on the `rx` channel into tokens and sends them out
/// on the `tx` channel.
fn tokenize_lines(rx: Receiver<String>, tx: Sender<Vec<String>>) {
    while let Ok(s) = rx.recv() {
        let t: Vec<_> = s.split_whitespace().map(String::from).collect();
        let _ = tx.send(t);
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
            Some(path) => PathBuf::from(path.to_lowercase()),
            None => {
                let e = String::from("Token vector is empty!");
                let _ = error_tx.send(e);
                continue;
            }
        };

        // Is the last argument in the compile command a file?
        let file_name = match arg_path.file_name() {
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
        if !arg_path.is_absolute() {
            if let Some(parent) = map.get(&file_name) {
                path = parent.clone();
                path.push(&file_name);
            };
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
                    test_path.push(&arg_path);

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
    process::exit(-1);
}

fn main() {
    //
    // Input validation
    //

    // Parse command line arguments
    let cli = Cli::parse();

    let package_name = env!("CARGO_PKG_NAME");
    let package_version = env!("CARGO_PKG_VERSION");

    // File reader
    let input_file_handle = match File::open(&cli.input_file) {
        Ok(handle) => BufReader::new(handle),
        Err(e) => exit_with_message(format!(
            "Failed to open {:?}: {}",
            cli.input_file, e
        )),
    };

    // Verify source directory is a valid path
    if !cli.source_directory.is_dir() {
        exit_with_message(format!(
            "Provided path is not a directory: {:?}",
            cli.source_directory
        ));
    }

    // File writer
    let output_file_handle = match File::options()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&cli.output_file)
    {
        Ok(handle) => handle,
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
        "{:^50}",
        format!("{package_name} v{package_version} - Run Start")
    );
    println!("==================================================");

    let start_time = Instant::now();
    //
    // Step 1
    //
    println!();
    println!("--------------------------------------------------");
    println!("{:^50}", "Generating the lookup tree");
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

        let handle = thread::spawn(move || {
            find_all_files(directory_rx, directory_tx, entry_tx, error_tx);
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
    println!("{:^50}", "Generating the database");
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
    thread::spawn(move || {
        find_all_lines(
            input_file_handle,
            &cli.compiler_executable,
            source_tx,
            e_tx,
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
    println!("{:^50}", "Writing the database to disk");
    println!("--------------------------------------------------");
    println!();
    println!("Output file: {:?}", cli.output_file);
    println!();

    let task_start_time = Instant::now();
    let _ = serde_json::to_writer_pretty(output_file_handle, &compile_commands);
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
    println!("{:^50}", "Run completed");
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
