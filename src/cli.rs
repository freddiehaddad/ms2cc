use clap::Parser;
use ms2cc::config::{self, DEFAULT_COMPILER_EXECUTABLE, DEFAULT_MAX_THREADS};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::num::NonZeroUsize;
use std::path::PathBuf;

pub const DEFAULT_BUFFER_SIZE: usize = 64 * 1024; // 64KB buffer for file I/O

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
    #[arg(short('x'), long, value_delimiter = ',')]
    exclude_directories: Option<Vec<String>>,

    /// File extensions to process (comma-separated)
    #[arg(short('e'), long, value_delimiter = ',')]
    file_extensions: Option<Vec<String>>,

    /// Name of compiler executable
    #[arg(short('c'), long, name = "EXE", default_value = DEFAULT_COMPILER_EXECUTABLE)]
    compiler_executable: String,

    /// Max number of threads per task
    #[arg(
        short('t'),
        long,
        default_value_t = NonZeroUsize::new(DEFAULT_MAX_THREADS).unwrap()
    )]
    max_threads: NonZeroUsize,
}

/// Validated application configuration derived from CLI arguments.
pub struct AppConfig {
    pub input_path: PathBuf,
    pub input_reader: BufReader<File>,
    pub output_path: PathBuf,
    pub output_writer: BufWriter<File>,
    pub pretty_print: bool,
    pub source_directory: PathBuf,
    pub exclude_directories: Vec<String>,
    pub file_extensions: Vec<String>,
    pub compiler_executable: String,
    pub max_threads: NonZeroUsize,
}

impl AppConfig {
    /// Parses command-line arguments and performs upfront validation, returning
    /// a fully-initialized application configuration or a human-readable error
    /// string suitable for printing to stderr.
    pub fn from_args() -> Result<Self, String> {
        Self::try_from_cli(Cli::parse())
    }

    fn try_from_cli(cli: Cli) -> Result<Self, String> {
        let Cli {
            input_file,
            output_file,
            pretty_print,
            source_directory,
            exclude_directories,
            file_extensions,
            compiler_executable,
            max_threads,
        } = cli;

        let input_path = input_file;
        let output_path = output_file;

        let input_handle = File::open(&input_path)
            .map_err(|e| format!("Failed to open {:?}: {}", input_path, e))?;
        let input_reader =
            BufReader::with_capacity(DEFAULT_BUFFER_SIZE, input_handle);

        if let Ok(metadata) = fs::metadata(&input_path)
            && metadata.len() == 0
        {
            return Err(format!("Input file {:?} is empty", input_path));
        }

        if !source_directory.is_dir() {
            return Err(format!(
                "Provided path is not a directory: {:?}",
                source_directory
            ));
        }

        if let Ok(mut entries) = fs::read_dir(&source_directory)
            && entries.next().is_none()
        {
            return Err(format!(
                "Source directory {:?} appears to be empty",
                source_directory
            ));
        }

        let output_file = File::options()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&output_path)
            .map_err(|e| format!("Failed to open {:?}: {}", output_path, e))?;
        let output_writer =
            BufWriter::with_capacity(DEFAULT_BUFFER_SIZE, output_file);

        let exclude_directories = exclude_directories
            .unwrap_or_else(config::Config::default_exclude_directories);
        let file_extensions = file_extensions
            .unwrap_or_else(config::Config::default_file_extensions);

        let compiler_executable = if compiler_executable.trim().is_empty() {
            DEFAULT_COMPILER_EXECUTABLE.to_string()
        } else {
            compiler_executable
        };

        Ok(Self {
            input_path,
            input_reader,
            output_path,
            output_writer,
            pretty_print,
            source_directory,
            exclude_directories,
            file_extensions,
            compiler_executable,
            max_threads,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, Cli};
    use ms2cc::config::{self, DEFAULT_COMPILER_EXECUTABLE};
    use std::fs::{self, File};
    use std::io::Write;
    use std::num::NonZeroUsize;
    use tempfile::TempDir;

    fn make_paths() -> (
        TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let temp_dir = TempDir::new().expect("create temp dir");

        let source_dir = temp_dir.path().join("src");
        fs::create_dir_all(&source_dir).expect("create source dir");
        File::create(source_dir.join("main.cpp")).expect("touch source file");

        let input_path = temp_dir.path().join("msbuild.log");
        {
            let mut handle =
                File::create(&input_path).expect("create log file");
            writeln!(handle, "cl.exe /c main.cpp").expect("write log");
        }

        let output_path = temp_dir.path().join("compile_commands.json");

        (temp_dir, input_path, output_path, source_dir)
    }

    #[test]
    fn try_from_cli_validates_and_constructs_config() {
        let (_temp_dir, input_path, output_path, source_dir) = make_paths();

        let cli = Cli {
            input_file: input_path.clone(),
            output_file: output_path.clone(),
            pretty_print: true,
            source_directory: source_dir.clone(),
            exclude_directories: Some(vec![
                ".git".to_string(),
                "build".to_string(),
            ]),
            file_extensions: Some(vec!["cpp".to_string(), "c".to_string()]),
            compiler_executable: "cl.exe".to_string(),
            max_threads: NonZeroUsize::new(2).unwrap(),
        };

        let AppConfig {
            input_path: config_input_path,
            mut input_reader,
            output_path: config_output_path,
            output_writer: _,
            pretty_print,
            source_directory: config_source_dir,
            exclude_directories,
            file_extensions,
            compiler_executable,
            max_threads,
        } = AppConfig::try_from_cli(cli).expect("config should succeed");

        assert_eq!(config_input_path, input_path);
        assert_eq!(config_output_path, output_path);
        assert!(pretty_print);
        assert_eq!(config_source_dir, source_dir);
        assert_eq!(exclude_directories, vec![".git", "build"]);
        assert_eq!(file_extensions, vec!["cpp", "c"]);
        assert_eq!(compiler_executable, "cl.exe");
        assert_eq!(max_threads.get(), 2);

        let mut contents = String::new();
        use std::io::Read;
        input_reader
            .read_to_string(&mut contents)
            .expect("read log");
        assert!(contents.contains("cl.exe"));
    }

    #[test]
    fn try_from_cli_applies_defaults() {
        let (_temp_dir, input_path, output_path, source_dir) = make_paths();

        let cli = Cli {
            input_file: input_path.clone(),
            output_file: output_path.clone(),
            pretty_print: false,
            source_directory: source_dir.clone(),
            exclude_directories: None,
            file_extensions: None,
            compiler_executable: String::new(),
            max_threads: NonZeroUsize::new(4).unwrap(),
        };

        let config =
            AppConfig::try_from_cli(cli).expect("config should succeed");

        assert_eq!(
            config.exclude_directories,
            config::Config::default_exclude_directories()
        );
        assert_eq!(
            config.file_extensions,
            config::Config::default_file_extensions()
        );
        assert_eq!(config.compiler_executable, DEFAULT_COMPILER_EXECUTABLE);
        assert_eq!(config.max_threads.get(), 4);
    }

    #[test]
    fn try_from_cli_rejects_empty_input_file() {
        let temp_dir = TempDir::new().expect("create temp dir");

        let source_dir = temp_dir.path().join("src");
        fs::create_dir_all(&source_dir).expect("create source dir");
        File::create(source_dir.join("main.cpp")).expect("touch source file");

        let input_path = temp_dir.path().join("empty.log");
        File::create(&input_path).expect("create empty log");

        let output_path = temp_dir.path().join("compile_commands.json");

        let cli = Cli {
            input_file: input_path.clone(),
            output_file: output_path,
            pretty_print: false,
            source_directory: source_dir,
            exclude_directories: None,
            file_extensions: None,
            compiler_executable: "cl.exe".to_string(),
            max_threads: NonZeroUsize::new(1).unwrap(),
        };

        let err = AppConfig::try_from_cli(cli)
            .err()
            .expect("empty input should fail");
        assert!(err.contains("empty"));
    }

    #[test]
    fn try_from_cli_rejects_empty_source_directory() {
        let temp_dir = TempDir::new().expect("create temp dir");

        let source_dir = temp_dir.path().join("src");
        fs::create_dir_all(&source_dir).expect("create empty source dir");

        let input_path = temp_dir.path().join("msbuild.log");
        {
            let mut handle =
                File::create(&input_path).expect("create log file");
            writeln!(handle, "cl.exe /c main.cpp").expect("write log");
        }

        let output_path = temp_dir.path().join("compile_commands.json");

        let cli = Cli {
            input_file: input_path,
            output_file: output_path,
            pretty_print: false,
            source_directory: source_dir,
            exclude_directories: None,
            file_extensions: None,
            compiler_executable: "cl.exe".to_string(),
            max_threads: NonZeroUsize::new(1).unwrap(),
        };

        let err = AppConfig::try_from_cli(cli)
            .err()
            .expect("empty source dir should fail");
        assert!(err.contains("appears to be empty"));
    }
}
