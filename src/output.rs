use anyhow::{Context, Result, bail};
#[cfg(not(windows))]
use std::fs;
use std::{
    io::{BufWriter, Write},
    path::Path,
};
use tempfile::NamedTempFile;

use crate::model::{CommandRecord, CompileCommand};
use crate::paths;

pub fn prepare(mut records: Vec<CommandRecord>) -> Result<Vec<CompileCommand>> {
    records.sort_by(|left, right| {
        left.command
            .file
            .to_ascii_lowercase()
            .cmp(&right.command.file.to_ascii_lowercase())
            .then_with(|| left.project.cmp(&right.project))
            .then_with(|| left.command.arguments.cmp(&right.command.arguments))
    });

    for record in &records {
        if record.command.directory.is_empty() {
            bail!("empty working directory for {}", record.command.file);
        }
        if record.command.file.is_empty() {
            bail!("compilation database entry has an empty source path");
        }
        if record.command.arguments.len() < 2 {
            bail!(
                "compiler argument vector is incomplete for {}",
                record.command.file
            );
        }
        #[cfg(windows)]
        {
            if !Path::new(&record.command.directory).is_absolute() {
                bail!(
                    "working directory is not absolute for {}: {}",
                    record.command.file,
                    record.command.directory
                );
            }
            if !Path::new(&record.command.file).is_absolute() {
                bail!("source path is not absolute: {}", record.command.file);
            }
            if !Path::new(&record.command.arguments[0]).is_absolute() {
                bail!(
                    "compiler path is not absolute for {}: {}",
                    record.command.file,
                    record.command.arguments[0]
                );
            }
        }
        if record
            .command
            .arguments
            .iter()
            .any(|argument| argument.contains("$(") || argument.contains("%("))
        {
            bail!(
                "unexpanded MSBuild expression remains in command for {}",
                record.command.file
            );
        }
        if !contains_source_argument(&record.command) {
            bail!(
                "compiler arguments do not contain their translation unit {}",
                record.command.file
            );
        }
    }
    Ok(records.into_iter().map(|record| record.command).collect())
}

fn contains_source_argument(command: &CompileCommand) -> bool {
    let expected = paths::key(Path::new(&command.file));
    command.arguments.iter().skip(1).any(|argument| {
        if paths::key(Path::new(argument)) == expected {
            return true;
        }
        if let Some(candidate_prefix) = argument.get(..3)
            && ["/Tc", "-Tc", "/Tp", "-Tp"]
                .iter()
                .any(|prefix| candidate_prefix.eq_ignore_ascii_case(prefix))
        {
            return paths::key(Path::new(&argument[3..])) == expected;
        }
        false
    })
}

pub fn write_atomic(path: &Path, commands: &[CompileCommand]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.is_dir() {
        bail!("output directory does not exist: {}", parent.display());
    }

    let mut temporary = NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create a temporary file in {}", parent.display()))?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        serde_json::to_writer_pretty(&mut writer, commands)
            .context("failed to serialize the compilation database")?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    temporary.as_file().sync_all()?;
    let (_, temporary_path) = temporary
        .keep()
        .context("failed to retain the completed temporary database")?;
    replace_file(&temporary_path, path).with_context(|| {
        format!(
            "failed to atomically replace {} with {}",
            path.display(),
            temporary_path.display()
        )
    })
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source: Vec<u16> = source.as_os_str().encode_wide().chain([0]).collect();
    let destination: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error()).context("MoveFileExW failed");
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    fs::rename(source, destination).context("rename failed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Properties;

    fn record(file: &str) -> CommandRecord {
        let file = format!(r"C:\project\{file}");
        CommandRecord {
            command: CompileCommand {
                directory: r"C:\project".to_owned(),
                arguments: vec![r"C:\tools\cl.exe".to_owned(), file.clone()],
                file,
            },
            project: r"C:\project\p.vcxproj".into(),
            properties: Properties::new(),
        }
    }

    #[test]
    fn sorts_commands_deterministically() {
        let output = prepare(vec![record("B.cpp"), record("a.cpp")]).unwrap();
        assert_eq!(output[0].file, r"C:\project\a.cpp");
        assert_eq!(output[1].file, r"C:\project\B.cpp");
    }
}
