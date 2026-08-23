use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    env,
    ffi::OsStr,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

static QUERY_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
enum Host {
    Executable(PathBuf),
    CommandScript(PathBuf),
}

#[derive(Debug, Clone)]
pub struct MsbuildRunner {
    host: Host,
    working_directory: PathBuf,
    temporary_directory: PathBuf,
    timeout: Duration,
}

impl MsbuildRunner {
    pub fn discover(
        explicit: Option<&Path>,
        working_directory: PathBuf,
        temporary_directory: PathBuf,
        timeout: Duration,
    ) -> Result<Self> {
        #[cfg(not(windows))]
        {
            let _ = (explicit, working_directory, temporary_directory, timeout);
            bail!("ms2cc supports Windows MSBuild hosts only");
        }

        #[cfg(windows)]
        {
            let path = if let Some(explicit) = explicit {
                resolve_explicit(explicit)?
            } else {
                discover_msbuild().context(
                    "could not find MSBuild; run from a developer/build shell or pass --msbuild",
                )?
            };
            let host = match path
                .extension()
                .and_then(OsStr::to_str)
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str()
            {
                "cmd" | "bat" => Host::CommandScript(path),
                _ => Host::Executable(path),
            };
            Ok(Self {
                host,
                working_directory,
                temporary_directory,
                timeout,
            })
        }
    }

    pub fn display_name(&self) -> String {
        match &self.host {
            Host::Executable(path) | Host::CommandScript(path) => path.display().to_string(),
        }
    }

    pub fn temporary_path(&self, name: impl AsRef<Path>) -> PathBuf {
        self.temporary_directory.join(name)
    }

    pub fn require_query_support(&self) -> Result<()> {
        let output = self.run(
            &["-nologo".to_owned(), "-version".to_owned()],
            "MSBuild version",
        )?;
        if !output.status.success() {
            bail!(
                "MSBuild version query failed with {}\n{}",
                output.status,
                format_process_output(&output.stdout, &output.stderr)
            );
        }
        let version = String::from_utf8_lossy(&output.stdout)
            .lines()
            .chain(String::from_utf8_lossy(&output.stderr).lines())
            .filter_map(parse_version)
            .next_back()
            .with_context(|| {
                format!(
                    "MSBuild did not report a parseable version\n{}",
                    format_process_output(&output.stdout, &output.stderr)
                )
            })?;
        if version.0 < 17 || (version.0 == 17 && version.1 < 8) {
            bail!(
                "MSBuild {}.{} is too old; ms2cc requires MSBuild 17.8 or newer",
                version.0,
                version.1
            );
        }
        Ok(())
    }

    pub fn query_json(&self, project: &Path, arguments: &[String], label: &str) -> Result<Value> {
        let id = QUERY_ID.fetch_add(1, Ordering::Relaxed);
        let result_path = self.temporary_directory.join(format!("query-{id}.json"));
        let mut all_arguments = Vec::with_capacity(arguments.len() + 6);
        all_arguments.push(project.display().to_string());
        all_arguments.push("-noAutoResponse".to_owned());
        all_arguments.push("-nologo".to_owned());
        all_arguments.push("-verbosity:quiet".to_owned());
        all_arguments.push("-nodeReuse:false".to_owned());
        all_arguments.extend_from_slice(arguments);
        all_arguments.push(format!("-getResultOutputFile:{}", result_path.display()));

        let output = self.run(&all_arguments, label)?;
        if !output.status.success() {
            bail!(
                "{label} failed for {} with {}\n{}",
                project.display(),
                output.status,
                format_process_output(&output.stdout, &output.stderr)
            );
        }
        let bytes = fs::read(&result_path).with_context(|| {
            format!(
                "{label} succeeded but did not produce {}",
                result_path.display()
            )
        })?;
        serde_json::from_slice(&bytes).with_context(|| {
            format!(
                "MSBuild wrote invalid query JSON to {}",
                result_path.display()
            )
        })
    }

    fn run(&self, arguments: &[String], label: &str) -> Result<CapturedOutput> {
        let id = QUERY_ID.fetch_add(1, Ordering::Relaxed);
        let response_path = self.temporary_directory.join(format!("args-{id}.rsp"));
        let response = arguments
            .iter()
            .map(|argument| quote_response_argument(argument))
            .collect::<Vec<_>>()
            .join("\r\n");
        fs::write(&response_path, response)
            .with_context(|| format!("failed to write {}", response_path.display()))?;

        let mut command = match &self.host {
            Host::Executable(path) => {
                let mut command = Command::new(path);
                command.arg(format!("@{}", response_path.display()));
                command
            }
            Host::CommandScript(path) => {
                let mut command = Command::new(path);
                command.arg(format!("@{}", response_path.display()));
                command
            }
        };
        command
            .current_dir(&self.working_directory)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_suspended(&mut command);

        let job = ProcessJob::new().context("failed to create the MSBuild process job")?;
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to start {label} using {}", self.display_name()))?;
        if let Err(error) = job.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error).context("failed to contain the MSBuild process tree");
        }
        if let Err(error) = resume_child(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error).context("failed to resume the contained MSBuild process");
        }
        let stdout = child
            .stdout
            .take()
            .context("failed to capture MSBuild stdout")?;
        let stderr = child
            .stderr
            .take()
            .context("failed to capture MSBuild stderr")?;
        let stdout_reader = thread::spawn(move || read_all(stdout));
        let stderr_reader = thread::spawn(move || read_all(stderr));

        let started = Instant::now();
        let mut next_heartbeat = Duration::from_secs(30);
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child.try_wait().context("failed to query MSBuild status")? {
                break status;
            }
            let elapsed = started.elapsed();
            if elapsed >= self.timeout {
                timed_out = true;
                job.terminate();
                let _ = child.kill();
                break child.wait().context("failed to reap timed-out MSBuild")?;
            }
            if elapsed >= next_heartbeat {
                eprintln!("{label} still running after {}s", elapsed.as_secs());
                next_heartbeat += Duration::from_secs(30);
            }
            thread::sleep(Duration::from_millis(100));
        };

        drop(job);
        let stdout = stdout_reader
            .join()
            .map_err(|_| anyhow::anyhow!("MSBuild stdout reader panicked"))??;
        let stderr = stderr_reader
            .join()
            .map_err(|_| anyhow::anyhow!("MSBuild stderr reader panicked"))??;
        if timed_out {
            bail!(
                "{label} exceeded the {} second timeout\n{}",
                self.timeout.as_secs(),
                format_process_output(&stdout, &stderr)
            );
        }
        Ok(CapturedOutput {
            status,
            stdout,
            stderr,
        })
    }
}

#[derive(Debug)]
struct CapturedOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn read_all(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn format_process_output(stdout: &[u8], stderr: &[u8]) -> String {
    const LIMIT: usize = 32 * 1024;
    let mut text = String::new();
    if !stdout.is_empty() {
        text.push_str("stdout:\n");
        text.push_str(&String::from_utf8_lossy(stdout));
    }
    if !stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str("stderr:\n");
        text.push_str(&String::from_utf8_lossy(stderr));
    }
    if text.len() > LIMIT {
        let mut start = text.len() - LIMIT;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        text.split_off(start)
    } else {
        text
    }
}

fn parse_version(line: &str) -> Option<(u32, u32)> {
    let line = line.trim();
    let mut parts = line.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    if parts.all(|part| part.chars().all(|character| character.is_ascii_digit())) {
        Some((major, minor))
    } else {
        None
    }
}

fn quote_response_argument(argument: &str) -> String {
    if !argument
        .chars()
        .any(|character| character.is_whitespace() || character == '"')
    {
        return argument.to_owned();
    }
    let mut result = String::from("\"");
    let mut backslashes = 0;
    for character in argument.chars() {
        match character {
            '\\' => backslashes += 1,
            '"' => {
                result.push_str(&"\\".repeat(backslashes * 2 + 1));
                result.push('"');
                backslashes = 0;
            }
            _ => {
                result.push_str(&"\\".repeat(backslashes));
                backslashes = 0;
                result.push(character);
            }
        }
    }
    result.push_str(&"\\".repeat(backslashes * 2));
    result.push('"');
    result
}

#[cfg(windows)]
fn resolve_explicit(path: &Path) -> Result<PathBuf> {
    if path.components().count() > 1 || path.is_absolute() {
        if path.is_file() {
            return Ok(if path.is_absolute() {
                path.to_path_buf()
            } else {
                env::current_dir()
                    .context("failed to determine the current directory")?
                    .join(path)
            });
        }
        bail!("MSBuild host does not exist: {}", path.display());
    }
    find_on_path(path.as_os_str()).with_context(|| {
        format!(
            "could not resolve MSBuild host '{}' through PATH",
            path.display()
        )
    })
}

#[cfg(windows)]
fn discover_msbuild() -> Option<PathBuf> {
    if let Some(value) = env::var_os("BUILD_MSBUILD_PROGRAM")
        && let Some(path) = resolve_environment_candidate(&value)
    {
        return Some(path);
    }
    if let Some(value) = env::var_os("MSBUILD_EXE_PATH") {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Some(path) = find_on_path(OsStr::new("msbuild")) {
        return Some(path);
    }

    let vswhere = PathBuf::from(
        env::var_os("ProgramFiles(x86)").unwrap_or_else(|| r"C:\Program Files (x86)".into()),
    )
    .join(r"Microsoft Visual Studio\Installer\vswhere.exe");
    if !vswhere.is_file() {
        return None;
    }
    let output = Command::new(vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.Component.MSBuild",
            "-find",
            r"MSBuild\**\Bin\MSBuild.exe",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

#[cfg(windows)]
fn resolve_environment_candidate(value: &OsStr) -> Option<PathBuf> {
    let path = PathBuf::from(value);
    if path.is_file() {
        Some(path)
    } else {
        find_on_path(value)
    }
}

#[cfg(windows)]
fn find_on_path(name: &OsStr) -> Option<PathBuf> {
    let name_path = Path::new(name);
    if name_path.is_file() {
        return Some(name_path.to_path_buf());
    }

    let has_extension = name_path.extension().is_some();
    let extensions: Vec<String> = if has_extension {
        vec![String::new()]
    } else {
        env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
            .split(';')
            .map(str::to_owned)
            .collect()
    };

    env::split_paths(&env::var_os("PATH")?).find_map(|directory| {
        extensions.iter().find_map(|extension| {
            let candidate = if extension.is_empty() {
                directory.join(name_path)
            } else {
                directory.join(format!("{}{}", name_path.to_string_lossy(), extension))
            };
            candidate.is_file().then_some(candidate)
        })
    })
}

#[cfg(windows)]
fn configure_suspended(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;

    command.creation_flags(CREATE_SUSPENDED);
}

#[cfg(not(windows))]
fn configure_suspended(_command: &mut Command) {}

#[cfg(windows)]
fn resume_child(child: &std::process::Child) -> Result<()> {
    use std::mem::size_of;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error()).context("CreateToolhelp32Snapshot failed");
    }
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<THREADENTRY32>() as u32;
    let mut found = false;
    let mut success = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while success {
        if entry.th32OwnerProcessID == child.id() {
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if thread.is_null() {
                unsafe { CloseHandle(snapshot) };
                return Err(std::io::Error::last_os_error()).context("OpenThread failed");
            }
            let resumed = unsafe { ResumeThread(thread) };
            unsafe { CloseHandle(thread) };
            if resumed == u32::MAX {
                unsafe { CloseHandle(snapshot) };
                return Err(std::io::Error::last_os_error()).context("ResumeThread failed");
            }
            found = true;
            break;
        }
        success = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    if !found {
        bail!("could not locate the suspended MSBuild process thread");
    }
    Ok(())
}

#[cfg(not(windows))]
fn resume_child(_child: &std::process::Child) -> Result<()> {
    Ok(())
}

#[cfg(windows)]
struct ProcessJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl ProcessJob {
    fn new() -> Result<Self> {
        use std::{mem::size_of, ptr};
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::JobObjects::{
                CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
        };

        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error()).context("CreateJobObjectW failed");
        }

        let mut information: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&information as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            unsafe { CloseHandle(handle) };
            return Err(std::io::Error::last_os_error()).context("SetInformationJobObject failed");
        }
        Ok(Self(handle))
    }

    fn assign(&self, child: &std::process::Child) -> Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

        let assigned = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as _) };
        if assigned == 0 {
            return Err(std::io::Error::last_os_error()).context("AssignProcessToJobObject failed");
        }
        Ok(())
    }

    fn terminate(&self) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessJob {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(not(windows))]
struct ProcessJob;

#[cfg(not(windows))]
impl ProcessJob {
    fn new() -> Result<Self> {
        Ok(Self)
    }

    fn assign(&self, _child: &std::process::Child) -> Result<()> {
        Ok(())
    }

    fn terminate(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_msbuild_versions() {
        assert_eq!(parse_version("17.14.23.42201"), Some((17, 14)));
        assert_eq!(parse_version("Microsoft Build Engine"), None);
        assert_eq!(parse_version("17.x"), None);
    }

    #[test]
    fn quotes_response_arguments() {
        assert_eq!(quote_response_argument("-nologo"), "-nologo");
        assert_eq!(
            quote_response_argument(r"C:\Program Files\MSBuild.exe"),
            r#""C:\Program Files\MSBuild.exe""#
        );
    }
}
