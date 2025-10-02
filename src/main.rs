//! Binary entry point that wires together filesystem traversal, log parsing,
//! and compile command generation for ms2cc.

mod cli;
mod command_builder;
mod fs_index;
mod log_scan;
mod pipeline;
use cli::AppConfig;
use pipeline::{PipelineConfig, PipelineCoordinator};
use serde_json::{to_writer, to_writer_pretty};
use std::process;
use std::time::Instant;

// Configuration constants
const EXIT_FAILURE: i32 = -1; // Exit code for failure
const HEADER_WIDTH: usize = 50; // Width for centered header text

/// Prints an error message to standard error and exits.
fn exit_with_message(msg: String) -> ! {
    eprintln!("{msg}");
    process::exit(EXIT_FAILURE);
}

// Orchestrates the CLI lifecycle: validate input, fan out worker threads, and
// write the resulting `compile_commands.json` file.
fn main() {
    //
    // Input validation
    //

    // Parse command line arguments and perform upfront validation
    let config = match AppConfig::from_args() {
        Ok(config) => config,
        Err(err) => exit_with_message(err),
    };

    let AppConfig {
        input_path,
        input_reader: log_reader,
        output_path,
        output_writer,
        pretty_print,
        source_directory,
        exclude_directories,
        file_extensions,
        compiler_executable,
        max_threads,
    } = config;

    let package_name = env!("CARGO_PKG_NAME");
    let package_version = env!("CARGO_PKG_VERSION");

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
    println!("Source directory: {:?}", source_directory);
    println!();
    println!("Starting threads:");
    println!(" - 1 filesystem traversal thread");
    println!(" - {} file processing threads", max_threads.get());
    println!();

    let pipeline_config = PipelineConfig {
        source_directory: source_directory.clone(),
        exclude_directories: exclude_directories.clone(),
        file_extensions: file_extensions.clone(),
        compiler_executable: compiler_executable.clone(),
        log_path: input_path.clone(),
        log_reader,
        max_threads,
    };

    let pipeline = PipelineCoordinator::new(pipeline_config);

    let lookup_result = pipeline.build_lookup_tree();
    for error in &lookup_result.errors {
        eprintln!("{error}");
    }
    println!(
        "Lookup tree generation completed in {:0.02} seconds",
        lookup_result.duration.as_secs_f32()
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
    println!("Input file: {:?}", input_path);
    println!();
    println!("Starting threads:");
    println!(" - 1 log scanning/tokenization thread");
    println!(" - 1 compile command generation thread");
    println!();

    let database_result =
        pipeline.generate_compile_commands(lookup_result.file_index.clone());
    for error in &database_result.errors {
        eprintln!("{error}");
    }

    if database_result.compile_commands.is_empty() {
        println!("Warning: No compile commands found in the log file");
    }
    println!(
        "Database generation completed in {:0.02} seconds",
        database_result.duration.as_secs_f32()
    );
    println!();

    let compile_commands = database_result.compile_commands;

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
    println!("Output file: {:?}", output_path);
    println!();

    let task_start_time = Instant::now();

    let writer_function = if pretty_print {
        to_writer_pretty
    } else {
        to_writer
    };

    if let Err(e) = writer_function(output_writer, &compile_commands) {
        let m = format!("Failed to write {:?}: {}", output_path, e);
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
    println!("Output location: {:?}", output_path);
    println!(
        "Total time elapsed: {:0.02} seconds",
        elapsed_time.as_secs_f32()
    );
    println!();
    println!("==================================================");
}
