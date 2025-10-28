use crossbeam_channel::{Receiver, unbounded};
use dashmap::DashMap;
use ms2cc::{CompileCommand, IndexedPath, Ms2ccError};
use std::cell::RefCell;
use std::io::BufRead;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::command_builder::CommandBuilder;
use crate::fs_index::FileWalker;
use crate::log_scan;

const RECV_TIMEOUT_MS: u64 = 500;

/// Configuration values required to execute the pipeline.
pub struct PipelineConfig<R> {
    pub source_directory: PathBuf,
    pub exclude_directories: Vec<String>,
    pub file_extensions: Vec<String>,
    pub compiler_executable: String,
    pub log_path: PathBuf,
    pub log_reader: R,
    pub max_threads: NonZeroUsize,
}

/// Coordinates execution of the filesystem indexing and log processing stages.
pub struct PipelineCoordinator<R>
where
    R: BufRead + Send + 'static,
{
    source_directory: PathBuf,
    exclude_directories: Vec<String>,
    file_extensions: Vec<String>,
    compiler_executable: String,
    log_path: PathBuf,
    log_reader: RefCell<Option<R>>,
    max_threads: NonZeroUsize,
}

/// Outcome of the filesystem indexing stage.
pub struct LookupResult {
    pub file_index: Arc<DashMap<PathBuf, IndexedPath>>,
    pub duration: Duration,
    pub errors: Vec<Ms2ccError>,
}

/// Outcome of the compile command generation stage.
pub struct DatabaseResult {
    pub compile_commands: Vec<CompileCommand>,
    pub duration: Duration,
    pub errors: Vec<Ms2ccError>,
}

impl<R> PipelineCoordinator<R>
where
    R: BufRead + Send + 'static,
{
    pub fn new(config: PipelineConfig<R>) -> Self {
        Self {
            source_directory: config.source_directory,
            exclude_directories: config.exclude_directories,
            file_extensions: config.file_extensions,
            compiler_executable: config.compiler_executable,
            log_path: config.log_path,
            log_reader: RefCell::new(Some(config.log_reader)),
            max_threads: config.max_threads,
        }
    }

    /// Executes the filesystem indexing stage, returning the populated file
    /// index together with timing and any collected errors.
    pub fn build_lookup_tree(&self) -> LookupResult {
        run_lookup_stage(
            self.source_directory.clone(),
            self.exclude_directories.clone(),
            self.file_extensions.clone(),
            self.max_threads,
        )
    }

    /// Executes the compile command generation stage using the supplied file
    /// index. The coordinator consumes the buffered log reader during this
    /// step.
    pub fn generate_compile_commands(
        &self,
        file_index: Arc<DashMap<PathBuf, IndexedPath>>,
    ) -> DatabaseResult {
        let reader = self
            .log_reader
            .borrow_mut()
            .take()
            .expect("log reader already consumed");

        run_database_stage(
            reader,
            self.log_path.clone(),
            self.compiler_executable.clone(),
            self.file_extensions.clone(),
            file_index,
        )
    }
}

fn run_lookup_stage(
    source_directory: PathBuf,
    exclude_directories: Vec<String>,
    file_extensions: Vec<String>,
    max_threads: NonZeroUsize,
) -> LookupResult {
    let start = Instant::now();

    let errors = Arc::new(Mutex::new(Vec::new()));
    let (error_tx, error_rx) = unbounded::<Ms2ccError>();
    let error_sink = Arc::clone(&errors);
    let error_handle = thread::spawn(move || {
        for error in error_rx.iter() {
            if let Ok(mut guard) = error_sink.lock() {
                guard.push(error);
            }
        }
    });

    let file_index: Arc<DashMap<PathBuf, IndexedPath>> =
        Arc::new(DashMap::new());
    let (entry_tx, entry_rx) = unbounded();

    let walker_dir = source_directory;
    let walker_excludes = exclude_directories;
    let walker_extensions = file_extensions;
    let walker_entry_tx = entry_tx.clone();
    let walker_error_tx = error_tx.clone();
    let walker_threads = max_threads.get();

    let walker_handle = thread::spawn(move || {
        let walker = FileWalker::new_with_threads(
            walker_dir,
            walker_excludes,
            walker_extensions,
            walker_threads,
        );

        for result in walker {
            match result {
                Ok(path) => {
                    if walker_entry_tx.send(path).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    if walker_error_tx.send(err).is_err() {
                        break;
                    }
                }
            }
        }
    });

    drop(entry_tx);

    let mut worker_handles = Vec::new();
    for _ in 0..max_threads.get() {
        let entry_rx = entry_rx.clone();
        let index = Arc::clone(&file_index);

        let handle = thread::spawn(move || {
            build_file_map(entry_rx, index);
        });

        worker_handles.push(handle);
    }

    drop(error_tx);

    let _ = walker_handle.join();
    for handle in worker_handles {
        let _ = handle.join();
    }

    let _ = error_handle.join();

    let errors = Arc::try_unwrap(errors).unwrap().into_inner().unwrap();

    let duration = start.elapsed();

    LookupResult {
        file_index,
        duration,
        errors,
    }
}

fn run_database_stage<R>(
    log_reader: R,
    log_path: PathBuf,
    compiler_executable: String,
    file_extensions: Vec<String>,
    file_index: Arc<DashMap<PathBuf, IndexedPath>>,
) -> DatabaseResult
where
    R: BufRead + Send + 'static,
{
    let start = Instant::now();

    let errors = Arc::new(Mutex::new(Vec::new()));
    let (error_tx, error_rx) = unbounded::<Ms2ccError>();
    let error_sink = Arc::clone(&errors);
    let error_handle = thread::spawn(move || {
        for error in error_rx.iter() {
            if let Ok(mut guard) = error_sink.lock() {
                guard.push(error);
            }
        }
    });

    let (token_tx, token_rx) = unbounded();
    let (command_tx, command_rx) = unbounded();

    let scan_error_tx = error_tx.clone();
    let scan_token_tx = token_tx.clone();
    let scanner_handle = thread::spawn(move || {
        let line_iter = log_scan::LogLineIter::new(
            log_reader,
            log_path,
            compiler_executable,
            file_extensions,
        );
        let token_iter = log_scan::TokenIter::new(line_iter);

        for item in token_iter {
            match item {
                Ok(tokens) => {
                    if scan_token_tx.send(tokens).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    if scan_error_tx.send(err).is_err() {
                        break;
                    }
                }
            }
        }
    });

    drop(token_tx);

    let builder_error_tx = error_tx.clone();
    let builder_handle = thread::spawn(move || {
        let builder = CommandBuilder::new(file_index);
        builder.run(token_rx, command_tx, builder_error_tx);
    });

    drop(error_tx);

    let compile_commands: Vec<CompileCommand> = command_rx.iter().collect();

    let _ = scanner_handle.join();
    let _ = builder_handle.join();
    let _ = error_handle.join();

    let errors = Arc::try_unwrap(errors).unwrap().into_inner().unwrap();

    let duration = start.elapsed();

    DatabaseResult {
        compile_commands,
        duration,
        errors,
    }
}

fn normalize_component(component: &std::ffi::OsStr) -> Option<PathBuf> {
    component.to_str().map(|s| PathBuf::from(s.to_lowercase()))
}

fn build_file_map(
    entry_rx: Receiver<PathBuf>,
    tree: Arc<DashMap<PathBuf, IndexedPath>>,
) {
    while let Ok(path) =
        entry_rx.recv_timeout(Duration::from_millis(RECV_TIMEOUT_MS))
    {
        if let (Some(file_name), Some(parent)) =
            (path.file_name(), path.parent())
            && let (Some(normalized_file_name), Some(normalized_parent)) = (
                normalize_component(file_name),
                normalize_component(parent.as_os_str()),
            )
        {
            tree.entry(normalized_file_name)
                .and_modify(|entry| entry.mark_conflict())
                .or_insert(IndexedPath::unique(normalized_parent));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PipelineConfig, PipelineCoordinator};
    use ms2cc::Ms2ccError;
    use std::fs::{self, File};
    use std::io::Cursor;
    use std::num::NonZeroUsize;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn make_config<R>(
        source_directory: PathBuf,
        log_reader: R,
        log_path: PathBuf,
    ) -> PipelineConfig<R> {
        PipelineConfig {
            source_directory,
            exclude_directories: Vec::new(),
            file_extensions: vec!["cpp".to_string()],
            compiler_executable: "cl.exe".to_string(),
            log_path,
            log_reader,
            max_threads: NonZeroUsize::new(2).unwrap(),
        }
    }

    #[test]
    fn pipeline_generates_compile_commands() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let source_root = temp_dir.path().join("src");
        fs::create_dir_all(&source_root).expect("create source dir");
        File::create(source_root.join("main.cpp")).expect("touch main.cpp");

        let log_contents = b"cl.exe /c main.cpp\n".to_vec();
        let log_reader = Cursor::new(log_contents);
        let log_path = temp_dir.path().join("msbuild.log");

        let config =
            make_config(temp_dir.path().to_path_buf(), log_reader, log_path);
        let pipeline = PipelineCoordinator::new(config);

        let lookup = pipeline.build_lookup_tree();
        assert!(lookup.errors.is_empty());
        assert_eq!(lookup.file_index.len(), 1);

        let database =
            pipeline.generate_compile_commands(Arc::clone(&lookup.file_index));
        assert!(database.errors.is_empty());
        assert_eq!(database.compile_commands.len(), 1);

        let command = &database.compile_commands[0];
        assert_eq!(command.file, PathBuf::from("main.cpp"));

        let expected_directory = PathBuf::from(
            source_root.to_str().expect("utf-8 path").to_lowercase(),
        );
        assert_eq!(command.directory, expected_directory);

        let arguments: Vec<String> = command
            .arguments
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(arguments, vec!["cl.exe", "/c", "main.cpp"]);
    }

    #[test]
    fn pipeline_collects_errors_from_database_stage() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let source_root = temp_dir.path().join("src");
        fs::create_dir_all(&source_root).expect("create source dir");
        File::create(source_root.join("main.cpp")).expect("touch main.cpp");

        let log_contents = b"cl.exe /c missing.cpp\n".to_vec();
        let log_reader = Cursor::new(log_contents);
        let log_path = temp_dir.path().join("msbuild.log");

        let config =
            make_config(temp_dir.path().to_path_buf(), log_reader, log_path);
        let pipeline = PipelineCoordinator::new(config);

        let lookup = pipeline.build_lookup_tree();
        assert!(lookup.errors.is_empty());

        let database =
            pipeline.generate_compile_commands(Arc::clone(&lookup.file_index));
        assert!(database.compile_commands.is_empty());
        assert!(
            database.errors.iter().any(|error| matches!(
                error,
                Ms2ccError::MissingFoArgument { .. }
            )),
            "expected missing /Fo argument error, found {:?}",
            database.errors
        );
    }
}
