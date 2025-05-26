use anyhow::{Context, Result, ensure};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, read_dir};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

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
    about = "Tool to generate a compile_commands.json file from an msbuild.log file."
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
    path: PathBuf,
    entry_tx: Sender<PathBuf>,
    error_tx: Sender<String>,
) {
    let mut stack = vec![path];
    while let Some(path) = stack.pop() {
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
                stack.push(path);
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
fn build_file_map(entry_rx: Receiver<PathBuf>) -> HashMap<PathBuf, PathBuf> {
    // Generate a map of files and their directories
    let mut tree = HashMap::new();
    while let Ok(path) = entry_rx.recv() {
        // Test if entry is a file with an extension
        if path.extension().is_some() {
            let file_name = PathBuf::from(path.file_name().unwrap());
            let parent = PathBuf::from(path.parent().unwrap());

            // Add KV pair (file/path) to the hash table; clear on collision
            tree.entry(file_name)
                .and_modify(|absolute_path: &mut PathBuf| {
                    *absolute_path = PathBuf::new()
                })
                .or_insert(parent);
        }
    }
    tree
}

/// Searches an `msbuild.log` for all lines containing `s` string and sends
/// them out on the `tx` channel.
fn find_all_lines(reader: BufReader<File>, s: &str, tx: Sender<String>) {
    reader.lines().map_while(Result::ok).for_each(|line| {
        if line.to_lowercase().contains(s) {
            let _ = tx.send(line);
        }
    });
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
    map: HashMap<PathBuf, PathBuf>,
    rx: Receiver<Vec<String>>,
    tx: Sender<CompileCommand>,
    error_tx: Sender<String>,
) {
    while let Ok(arguments) = rx.recv() {
        // The file name should be the last compiler argument
        let mut path = match arguments.last() {
            Some(path) => PathBuf::from(path.to_lowercase()),
            None => {
                let e = String::from("Token vector is empty!");
                let _ = error_tx.send(e);
                continue;
            }
        };

        // Is the last argument in the compile command a file?
        let file_name = match path.file_name() {
            Some(file_name) => PathBuf::from(file_name),
            None => {
                let e = format!("Missing file_name component in {path:?}");
                let _ = error_tx.send(e);
                continue;
            }
        };

        // Does it have an extension?
        if file_name.extension().is_none() {
            let e = format!("File name missing extension {file_name:?}");
            let _ = error_tx.send(e);
            continue;
        }

        // If we only have a file name or relative path, try to reconstruct an
        // absolute path.

        // First we check our directory tree
        if !path.is_absolute() {
            if let Some(parent) = map.get(&file_name) {
                path = parent.clone();
                path.push(&file_name);
            };
        }

        // Last option is to try and reconstruct the path from the /Fo argument.
        if !path.is_absolute() {
            let argument = "/Fo";
            if let Some(fo_argument) =
                arguments.iter().find(|s| s.starts_with(argument))
            {
                path = PathBuf::from(
                    fo_argument.strip_prefix(argument).unwrap().to_lowercase(),
                );

                while path.has_root() {
                    path.push(&file_name);

                    // Did we find the path?
                    if path.is_file() {
                        break;
                    }
                    path.pop();

                    // Reached the end?
                    if !path.pop() {
                        break;
                    }
                }
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

fn main() -> Result<()> {
    // Parse command line arguments
    let cli = Cli::parse();

    let package_name = env!("CARGO_PKG_NAME");
    let package_version = env!("CARGO_PKG_VERSION");
    println!("{package_name} v{package_version}");

    // File reader
    let input_file_handle = BufReader::new(
        File::open(&cli.input_file)
            .with_context(|| format!("Failed to open {:?}", cli.input_file))?,
    );

    // Verify source directory is a valid path
    ensure!(
        cli.source_directory.is_dir(),
        format!(
            "Provided path is not a directory: {:?}",
            cli.source_directory
        )
    );

    // File writer
    let output_file_handle = File::options()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&cli.output_file)
        .with_context(|| format!("Failed to open {:?}", cli.output_file))?;

    println!(
        "Preparing to generate the lookup tree (this will take some time) ..."
    );
    let tree = thread::scope(|s| {
        let (entry_tx, entry_rx) = channel();
        let (error_tx, error_rx) = channel();

        // Separate thread for error handling.
        s.spawn(move || {
            println!("Error handling thread initialized.");
            error_handler(error_rx);
        });

        // Process discovered files
        let h = s.spawn(move || {
            println!("Tree generating thread initilized.");
            // Value is returned by the thread
            build_file_map(entry_rx)
        });

        // Traverse the directory tree
        println!("Directory thraversal thread initialized.");
        find_all_files(cli.source_directory, entry_tx, error_tx);

        // Return the tree to the main thread
        h.join().unwrap()
    });
    println!("Finished");

    println!(
        "Preparing to generate {:?} (this will take some time) ...",
        cli.output_file
    );
    thread::scope(|s| {
        let (source_tx, source_rx) = channel();
        let (preprocess_tx, preprocess_rx) = channel();
        let (token_tx, token_rx) = channel();
        let (compile_command_tx, compile_command_rx) = channel();
        let (error_tx, error_rx) = channel();

        // Separate thread for error handling.
        s.spawn(move || {
            println!("Error handling thread initialized.");
            error_handler(error_rx);
        });

        // Collect all the compile commands from the input file
        s.spawn(move || {
            println!("Log searching thread initialized.");
            println!(
                "Scanning {:?} (this will take some time) ...",
                cli.input_file
            );
            find_all_lines(
                input_file_handle,
                &cli.compiler_executable,
                source_tx,
            );
        });

        // Remove nested quotes (")
        s.spawn(move || {
            println!("Log entry cleanup thread initialized.");
            cleanup_line(source_rx, preprocess_tx);
        });

        // Tokenize
        s.spawn(move || {
            println!("Log entry tokenization thread initialized.");
            tokenize_lines(preprocess_rx, token_tx);
        });

        // Verify the input
        s.spawn(move || {
            println!("Compile command generation thread initialized.");
            create_compile_commands(
                tree,
                token_rx,
                compile_command_tx,
                error_tx,
            );
        });

        // Generate the compile_commands.json file
        println!("Waiting for compile commands ...",);
        let compile_commands: Vec<_> = compile_command_rx.iter().collect();
        println!("Writing {:?} database ...", cli.output_file);
        let _ =
            serde_json::to_writer_pretty(output_file_handle, &compile_commands);
    });
    println!("Finished");

    Ok(())
}
