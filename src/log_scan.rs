use ms2cc::{Ms2ccError, parser};
use std::io::BufRead;
use std::path::PathBuf;

const MULTILINE_RESERVE_SIZE: usize = 512;

/// Iterator that yields compile-command lines discovered in an MSBuild log.
/// Each item is either a command line containing `compiler_executable` or an
/// error explaining why log scanning failed.
pub struct LogLineIter<R: BufRead> {
    reader: R,
    log_path: PathBuf,
    compiler_exe_lower: String,
    file_extensions: Vec<String>,
    line_buffer: String,
    pending_command: Option<String>,
}

impl<R: BufRead> LogLineIter<R> {
    pub fn new(
        reader: R,
        log_path: impl Into<PathBuf>,
        compiler_executable: impl Into<String>,
        file_extensions: Vec<String>,
    ) -> Self {
        Self {
            reader,
            log_path: log_path.into(),
            compiler_exe_lower: compiler_executable.into().to_lowercase(),
            file_extensions,
            line_buffer: String::new(),
            pending_command: None,
        }
    }
}

impl<R: BufRead> Iterator for LogLineIter<R> {
    type Item = Result<String, Ms2ccError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            self.line_buffer.clear();
            match self.reader.read_line(&mut self.line_buffer) {
                Ok(0) => return None,
                Ok(_) => {
                    let line = self
                        .line_buffer
                        .trim_end_matches(['\r', '\n'])
                        .to_string();
                    let lowercase = line.to_lowercase();

                    if let Some(command) = self.pending_command.as_mut() {
                        command.push(' ');
                        command.push_str(&line);

                        if parser::ends_with_cpp_source_file(
                            &line,
                            &self.file_extensions,
                        ) {
                            let full = self.pending_command.take().unwrap();
                            return Some(Ok(full));
                        }

                        if lowercase.contains(&self.compiler_exe_lower) {
                            let error = Ms2ccError::UnexpectedLine {
                                line,
                                current: command.clone(),
                            };
                            self.pending_command = None;
                            return Some(Err(error));
                        }

                        continue;
                    }

                    let Some(executable) = first_executable_name(&lowercase)
                    else {
                        continue;
                    };

                    if executable != self.compiler_exe_lower {
                        continue;
                    }

                    if parser::ends_with_cpp_source_file(
                        &line,
                        &self.file_extensions,
                    ) {
                        return Some(Ok(line));
                    }

                    let mut command = line;
                    command.reserve(MULTILINE_RESERVE_SIZE);
                    self.pending_command = Some(command);
                }
                Err(err) => {
                    return Some(Err(Ms2ccError::LogRead {
                        source: err,
                        path: self.log_path.clone(),
                    }));
                }
            }
        }
    }
}

/// Iterator adaptor that tokenizes compile-command strings while preserving
/// error values from the underlying iterator.
pub struct TokenIter<I> {
    inner: I,
}

impl<I> TokenIter<I> {
    pub fn new(inner: I) -> Self {
        Self { inner }
    }
}

impl<I> Iterator for TokenIter<I>
where
    I: Iterator<Item = Result<String, Ms2ccError>>,
{
    type Item = Result<Vec<String>, Ms2ccError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|item| {
            item.map(|line| parser::tokenize_compile_command(&line))
        })
    }
}

fn first_executable_name(line: &str) -> Option<String> {
    line.match_indices(".exe").next().map(|(idx, _)| {
        let prefix = &line[..=idx + 3];
        let start = prefix
            .rfind(['\\', '/', ' ', '\t', '"', '\'', '('])
            .map(|pos| pos + 1)
            .unwrap_or(0);
        prefix[start..].trim_matches(['"', '\'']).to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::{LogLineIter, TokenIter, first_executable_name};
    use ms2cc::Ms2ccError;
    use std::io::Cursor;

    fn make_reader(lines: &[&str]) -> Cursor<Vec<u8>> {
        Cursor::new(lines.join("\n").into_bytes())
    }

    #[test]
    fn detects_single_line_commands() {
        let lines = ["cl.exe /c main.cpp"];
        let reader = make_reader(&lines);

        let mut iter = LogLineIter::new(
            reader,
            "log.txt",
            "cl.exe",
            vec!["cpp".to_string()],
        );

        let command =
            iter.next().transpose().expect("command present").unwrap();
        assert_eq!(command, "cl.exe /c main.cpp");
        assert!(iter.next().is_none());
    }

    #[test]
    fn merges_multi_line_commands() {
        let lines = ["cl.exe /c", "  /Iinc", "  main.cpp"];
        let reader = make_reader(&lines);

        let mut iter = LogLineIter::new(
            reader,
            "log.txt",
            "cl.exe",
            vec!["cpp".to_string()],
        );

        let command =
            iter.next().transpose().expect("command present").unwrap();
        assert_eq!(command, "cl.exe /c   /Iinc   main.cpp");
    }

    #[test]
    fn surfaces_unexpected_lines() {
        let lines = ["cl.exe /c", "cl.exe /nologo"];
        let reader = make_reader(&lines);

        let mut iter = LogLineIter::new(
            reader,
            "log.txt",
            "cl.exe",
            vec!["cpp".to_string()],
        );

        let err = iter.next().transpose().expect_err("expected error");
        assert!(matches!(err, Ms2ccError::UnexpectedLine { .. }));
    }

    #[test]
    fn token_iter_preserves_errors() {
        let lines = ["cl.exe /c main.cpp"];
        let reader = make_reader(&lines);

        let iter = LogLineIter::new(
            reader,
            "log.txt",
            "cl.exe",
            vec!["cpp".to_string()],
        );
        let mut tokens = TokenIter::new(iter);

        let command_tokens =
            tokens.next().transpose().expect("tokens").unwrap();
        assert_eq!(command_tokens, vec!["cl.exe", "/c", "main.cpp"]);
    }

    #[test]
    fn detects_executable_name() {
        assert_eq!(
            first_executable_name(" C:/Toolchains/cl.exe /c"),
            Some("cl.exe".to_string())
        );
    }
}
