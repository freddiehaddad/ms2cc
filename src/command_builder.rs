use crossbeam_channel::{Receiver, Sender};
use dashmap::DashMap;
use ms2cc::{
    CompileCommand, IndexedPath, Ms2ccError, compile_commands, parser,
};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Consumes tokenized compiler invocations and converts them into
/// `CompileCommand` records using previously indexed filesystem state.
pub struct CommandBuilder {
    file_index: Arc<DashMap<PathBuf, IndexedPath>>,
}

impl CommandBuilder {
    /// Creates a new `CommandBuilder` backed by the given file index.
    pub fn new(file_index: Arc<DashMap<PathBuf, IndexedPath>>) -> Self {
        Self { file_index }
    }

    /// Processes a single token vector and returns a sequence of results in the
    /// order they were encountered. Successful entries yield `CompileCommand`
    /// values while failures emit structured `Ms2ccError` diagnostics.
    pub fn build(
        &self,
        tokens: Vec<String>,
    ) -> Vec<Result<CompileCommand, Ms2ccError>> {
        assemble(&self.file_index, tokens)
    }

    /// Consumes tokens from `token_rx` and forwards the resulting compile
    /// commands or errors to the provided channels. The method preserves
    /// emission order so downstream consumers observe events exactly as they
    /// were produced.
    pub fn run(
        self,
        token_rx: Receiver<Vec<String>>,
        command_tx: Sender<CompileCommand>,
        error_tx: Sender<Ms2ccError>,
    ) {
        for tokens in token_rx.iter() {
            let events = self.build(tokens);
            for event in events {
                match event {
                    Ok(command) => {
                        if command_tx.send(command).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        if error_tx.send(error).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    }
}

fn assemble(
    file_index: &Arc<DashMap<PathBuf, IndexedPath>>,
    tokens: Vec<String>,
) -> Vec<Result<CompileCommand, Ms2ccError>> {
    if tokens.is_empty() {
        return vec![Err(Ms2ccError::EmptyTokenVector)];
    }

    let arguments: Vec<OsString> =
        tokens.into_iter().map(OsString::from).collect();

    let mut trailing_indices: Vec<usize> = arguments
        .iter()
        .enumerate()
        .rev()
        .take_while(|(_, arg)| is_source_file_argument(arg.as_os_str()))
        .map(|(idx, _)| idx)
        .collect();
    trailing_indices.reverse();

    if trailing_indices.is_empty() {
        return vec![Err(Ms2ccError::MissingTrailingFile { arguments })];
    }

    let mut results = Vec::with_capacity(trailing_indices.len());

    for index in trailing_indices {
        let file_argument = arguments[index].as_os_str();
        match resolve_source_path(file_index, &arguments, file_argument) {
            Ok(path) => match compile_commands::create_compile_command(
                path,
                arguments.clone(),
            ) {
                Ok(command) => results.push(Ok(command)),
                Err(err) => results.push(Err(err)),
            },
            Err(err) => results.push(Err(err)),
        }
    }

    results
}

fn is_source_file_argument(arg: &OsStr) -> bool {
    parser::extract_and_validate_filename(Path::new(arg)).is_ok()
}

fn resolve_source_path(
    file_index: &Arc<DashMap<PathBuf, IndexedPath>>,
    arguments: &[OsString],
    file_argument: &OsStr,
) -> Result<PathBuf, Ms2ccError> {
    let arg_path_buf = PathBuf::from(file_argument);
    let file_name = parser::extract_and_validate_filename(&arg_path_buf)?;

    let mut path = if arg_path_buf.is_absolute() {
        arg_path_buf.clone()
    } else if let Some(parent) = file_index
        .get(&file_name)
        .and_then(|entry| entry.value().parent().cloned())
    {
        let mut parent_path = parent;
        parent_path.push(&file_name);
        parent_path
    } else {
        PathBuf::new()
    };

    if !path.is_absolute() {
        if let Some(fo_argument) = compile_commands::find_fo_argument(arguments)
        {
            let mut fo_path =
                compile_commands::extract_fo_path(fo_argument.as_os_str())?;

            while fo_path.has_root() {
                let mut test_path = fo_path.clone();
                test_path.push(&file_name);
                if test_path.is_file() {
                    path = test_path;
                    break;
                }

                test_path.pop();
                test_path.push(&arg_path_buf);
                if test_path.is_file() {
                    path = test_path;
                    break;
                }

                if !fo_path.pop() {
                    break;
                }
            }

            if path.as_os_str().is_empty() {
                path = fo_path;
            }
        } else {
            return Err(Ms2ccError::MissingFoArgument {
                arguments: arguments.to_vec(),
            });
        }
    }

    if !path.is_absolute() || !path.is_file() {
        return Err(Ms2ccError::UnresolvedSourcePath {
            file: file_name,
            arguments: arguments.to_vec(),
        });
    }

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::CommandBuilder;
    use dashmap::DashMap;
    use ms2cc::{CompileCommand, IndexedPath, Ms2ccError};
    use std::fs::File;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[test]
    fn processes_multiple_trailing_files() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let source_dir = temp_dir.path();
        let files = ["main.cpp", "foo.cpp", "bar.cpp"];

        for file in &files {
            let path = source_dir.join(file);
            let mut handle = File::create(&path).expect("create source file");
            writeln!(handle, "int main() {{ return 0; }}").unwrap();
        }

        let map: Arc<DashMap<PathBuf, IndexedPath>> = Arc::new(DashMap::new());
        for file in &files {
            map.insert(
                PathBuf::from(file),
                IndexedPath::unique(source_dir.to_path_buf()),
            );
        }

        let builder = CommandBuilder::new(Arc::clone(&map));

        let mut command = vec![
            "cl.exe".to_string(),
            "/c".to_string(),
            "/DDEBUG".to_string(),
        ];
        command.extend(files.iter().map(|file| file.to_string()));

        let results = builder.build(command);
        assert_eq!(results.len(), files.len());

        let mut expected = vec![
            "cl.exe".to_string(),
            "/c".to_string(),
            "/DDEBUG".to_string(),
        ];
        expected.extend(files.iter().map(|file| file.to_string()));

        for (result, file) in results.iter().zip(files.iter()) {
            match result {
                Ok(CompileCommand {
                    file: result_file,
                    directory,
                    arguments,
                }) => {
                    assert_eq!(directory, source_dir);
                    assert_eq!(result_file.to_str().unwrap(), *file);
                    let args: Vec<String> = arguments
                        .iter()
                        .map(|arg| arg.to_string_lossy().into_owned())
                        .collect();
                    assert_eq!(args, expected);
                }
                Err(err) => panic!("Unexpected error: {err}"),
            }
        }
    }

    #[test]
    fn reports_missing_tokens() {
        let map: Arc<DashMap<PathBuf, IndexedPath>> = Arc::new(DashMap::new());
        let builder = CommandBuilder::new(map);

        let results = builder.build(Vec::new());
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], Err(Ms2ccError::EmptyTokenVector)));
    }

    #[test]
    fn reports_missing_trailing_file() {
        let map: Arc<DashMap<PathBuf, IndexedPath>> = Arc::new(DashMap::new());
        let builder = CommandBuilder::new(map);

        let results =
            builder.build(vec!["cl.exe".to_string(), "/c".to_string()]);
        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0],
            Err(Ms2ccError::MissingTrailingFile { .. })
        ));
    }
}
