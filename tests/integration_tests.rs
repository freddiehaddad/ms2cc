#![cfg(windows)]

use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ms2cc"))
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("native")
        .join(name)
}

fn run(arguments: &[&str]) -> Output {
    Command::new(binary())
        .args(arguments)
        .output()
        .expect("failed to run ms2cc")
}

fn run_in(directory: &Path, arguments: &[&str]) -> Output {
    Command::new(binary())
        .current_dir(directory)
        .args(arguments)
        .output()
        .expect("failed to run ms2cc")
}

fn read_database(path: &Path) -> Vec<Value> {
    serde_json::from_slice::<Value>(&fs::read(path).expect("failed to read database"))
        .expect("invalid JSON database")
        .as_array()
        .expect("database is not an array")
        .clone()
}

fn copy_native_fixture(destination: &Path) {
    fs::create_dir_all(destination.join("basic")).unwrap();
    fs::create_dir_all(destination.join("basic\\real")).unwrap();
    fs::create_dir_all(destination.join("include")).unwrap();
    for relative in [
        "settings.props",
        "traversal.proj",
        "nested.proj",
        "basic\\basic.vcxproj",
        "basic\\generated.cpp",
        "basic\\original-after.targets",
        "basic\\pch.cpp",
        "basic\\one.cpp",
        "basic\\real\\__temporary.cpp",
        "basic\\space name.cpp",
        "include\\pch.h",
    ] {
        fs::copy(fixture(relative), destination.join(relative)).unwrap();
    }
}

fn copy_reference_fixture(destination: &Path) {
    for directory in ["consumer", "library", "library\\include", "excluded"] {
        fs::create_dir_all(destination.join(directory)).unwrap();
    }
    for relative in [
        "references.sln",
        "consumer\\consumer.vcxproj",
        "consumer\\consumer.cpp",
        "library\\library.vcxproj",
        "library\\library.cpp",
        "library\\math.ixx",
        "library\\include\\library.h",
        "excluded\\excluded.vcxproj",
        "excluded\\excluded.cpp",
    ] {
        fs::copy(
            fixture(&format!("references\\{relative}")),
            destination.join(relative),
        )
        .unwrap();
    }
}

#[test]
fn generates_complete_database_from_vcxproj() {
    let temp = TempDir::new().unwrap();
    let output = temp.path().join("compile_commands.json");
    let result = run(&[
        fixture("basic\\basic.vcxproj").to_str().unwrap(),
        "--configuration",
        "Release",
        "--platform",
        "x64",
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    let database = read_database(&output);
    assert_eq!(database.len(), 5);
    let files: Vec<_> = database
        .iter()
        .map(|entry| entry["file"].as_str().unwrap().to_ascii_lowercase())
        .collect();
    assert!(files.iter().any(|file| file.ends_with(r"\pch.cpp")));
    assert!(files.iter().any(|file| file.ends_with(r"\one.cpp")));
    assert!(files.iter().any(|file| file.ends_with(r"\generated.cpp")));
    assert!(
        files
            .iter()
            .any(|file| file.ends_with(r"\real\__temporary.cpp"))
    );
    assert!(files.iter().any(|file| file.ends_with(r"\space name.cpp")));

    for entry in &database {
        let arguments = entry["arguments"].as_array().unwrap();
        assert!(Path::new(arguments[0].as_str().unwrap()).is_file());
        assert!(arguments.iter().any(|argument| {
            argument
                .as_str()
                .is_some_and(|value| value.starts_with("/Fo"))
        }));
        assert!(arguments.iter().any(|argument| {
            argument
                .as_str()
                .is_some_and(|value| value.starts_with("/Fp"))
        }));
        assert!(arguments.iter().any(|argument| {
            argument
                .as_str()
                .is_some_and(|value| value.eq_ignore_ascii_case("/FI"))
                || argument
                    .as_str()
                    .is_some_and(|value| value.starts_with("/FI"))
        }));
        assert!(arguments.iter().any(|argument| {
            argument
                .as_str()
                .is_some_and(|value| value == "MS2CC_IMPORTED_AFTER=1")
        }));
        assert_eq!(
            arguments
                .iter()
                .filter(|argument| {
                    argument.as_str().is_some_and(|value| {
                        [".c", ".cc", ".cpp", ".cxx"]
                            .iter()
                            .any(|extension| value.to_ascii_lowercase().ends_with(extension))
                    })
                })
                .count(),
            1
        );
    }

    let per_file = database
        .iter()
        .find(|entry| {
            entry["file"]
                .as_str()
                .is_some_and(|file| file.ends_with("space name.cpp"))
        })
        .unwrap();
    assert!(
        per_file["arguments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|argument| {
                argument
                    .as_str()
                    .is_some_and(|value| value == "MS2CC_PER_FILE=1")
            })
    );
}

#[test]
fn preserves_a_real_root_source_named_temporary() {
    let temp = TempDir::new().unwrap();
    let output = temp.path().join("compile_commands.json");
    let result = run(&[
        fixture("temporary-name\\temporary-name.vcxproj")
            .to_str()
            .unwrap(),
        "--configuration",
        "Release",
        "--platform",
        "x64",
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let database = read_database(&output);
    assert_eq!(database.len(), 1);
    assert!(
        database[0]["file"]
            .as_str()
            .is_some_and(|file| file.ends_with(r"\__temporary.cpp"))
    );
}

#[test]
fn resolves_header_unit_commands_without_leaving_scan_outputs() {
    let temp = TempDir::new().unwrap();
    let output = temp.path().join("compile_commands.json");
    let project = fixture("modules\\modules.vcxproj");
    let result = run(&[
        project.to_str().unwrap(),
        "--configuration",
        "Release",
        "--platform",
        "x64",
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    let database = read_database(&output);
    assert_eq!(database.len(), 2);
    let header = database
        .iter()
        .find(|entry| {
            entry["file"]
                .as_str()
                .is_some_and(|file| file.ends_with("widget.h"))
        })
        .unwrap();
    let header_arguments: Vec<_> = header["arguments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|argument| argument.as_str().unwrap())
        .collect();
    assert!(
        !header_arguments
            .iter()
            .any(|argument| argument.eq_ignore_ascii_case("/scanModules"))
    );
    assert!(header_arguments.iter().any(|argument| {
        argument
            .strip_prefix("/Fo")
            .is_some_and(|path| path.ends_with(r"\obj\widget.h.obj"))
    }));
    assert!(header_arguments.windows(2).any(|pair| {
        pair[0].eq_ignore_ascii_case("/ifcOutput") && pair[1].ends_with(r"\obj\widget.h.ifc")
    }));
    assert!(header_arguments.windows(2).any(|pair| {
        pair[0].eq_ignore_ascii_case("/sourceDependencies")
            && pair[1].ends_with(r"\obj\widget.h.ifc.d.json")
    }));

    let consumer = database
        .iter()
        .find(|entry| {
            entry["file"]
                .as_str()
                .is_some_and(|file| file.ends_with("main.cpp"))
        })
        .unwrap();
    let consumer_arguments: Vec<_> = consumer["arguments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|argument| argument.as_str().unwrap())
        .collect();
    assert!(consumer_arguments.windows(2).any(|pair| {
        pair[0].eq_ignore_ascii_case("/headerUnit")
            && pair[1].contains("widget.h=")
            && pair[1].ends_with(r"\obj\widget.h.ifc")
    }));
    assert!(
        !project
            .parent()
            .unwrap()
            .join("obj")
            .join("widget.h.module.json")
            .exists()
    );
}

#[test]
fn traversal_project_discovers_native_projects() {
    let temp = TempDir::new().unwrap();
    let output = temp.path().join("compile_commands.json");
    let result = run(&[
        fixture("traversal.proj").to_str().unwrap(),
        "--configuration",
        "Release",
        "--platform",
        "x64",
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(read_database(&output).len(), 5);
}

#[test]
fn prepared_project_references_preserve_public_includes() {
    let temp = TempDir::new().unwrap();
    let output = temp.path().join("compile_commands.json");
    let result = run(&[
        fixture("references\\consumer\\consumer.vcxproj")
            .to_str()
            .unwrap(),
        "--configuration",
        "Release",
        "--platform",
        "x64",
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    let database = read_database(&output);
    assert_eq!(database.len(), 5);
    let consumer = database
        .iter()
        .find(|entry| {
            entry["file"]
                .as_str()
                .is_some_and(|file| file.ends_with("consumer.cpp"))
        })
        .unwrap();
    assert!(
        consumer["arguments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|argument| {
                argument.as_str().is_some_and(|value| {
                    value
                        .to_ascii_lowercase()
                        .ends_with(r"\references\library\include")
                })
            })
    );
    assert!(
        consumer["arguments"]
            .as_array()
            .unwrap()
            .windows(2)
            .any(|pair| {
                pair[0]
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case("/reference"))
                    && pair[1].as_str().is_some_and(|value| {
                        value.starts_with("math=") && value.ends_with("math.ixx.ifc")
                    })
            })
    );
    assert!(
        consumer["arguments"]
            .as_array()
            .unwrap()
            .windows(2)
            .any(|pair| {
                pair[0]
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case("/headerUnit"))
                    && pair[1].as_str().is_some_and(|value| {
                        value.contains("library.h=") && value.ends_with("library.h.ifc")
                    })
            })
    );
}

#[test]
fn solution_build_rules_are_preserved_while_following_references() {
    for solution in ["references.sln", "references.slnx"] {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("compile_commands.json");
        let result = run(&[
            fixture(&format!("references\\{solution}"))
                .to_str()
                .unwrap(),
            "--configuration",
            "Release",
            "--platform",
            "x64",
            "--output",
            output.to_str().unwrap(),
        ]);
        assert!(
            result.status.success(),
            "{solution}: {}",
            String::from_utf8_lossy(&result.stderr)
        );

        let database = read_database(&output);
        assert_eq!(database.len(), 4, "{solution}");
        assert!(!database.iter().any(|entry| {
            entry["file"]
                .as_str()
                .is_some_and(|file| file.ends_with("excluded.cpp"))
        }));
    }
}

#[test]
fn solution_formats_discover_native_projects() {
    for solution in ["basic.sln", "basic.slnx"] {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("compile_commands.json");
        let result = run(&[
            fixture(solution).to_str().unwrap(),
            "--configuration",
            "Release",
            "--platform",
            "x64",
            "--output",
            output.to_str().unwrap(),
        ]);
        assert!(
            result.status.success(),
            "{solution}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let database = read_database(&output);
        assert_eq!(database.len(), 5, "{solution}");
        for entry in database {
            let arguments = entry["arguments"].as_array().unwrap();
            assert!(arguments.iter().any(|argument| {
                argument
                    .as_str()
                    .is_some_and(|value| value == "MS2CC_SOLUTION_NAME=basic")
            }));
            assert!(arguments.iter().any(|argument| {
                argument
                    .as_str()
                    .is_some_and(|value| value.to_ascii_lowercase().ends_with(r"\solution-include"))
            }));
        }
    }
}

#[test]
fn solution_excludes_projects_not_selected_for_build() {
    let temp = TempDir::new().unwrap();
    let result = run(&[
        fixture("basic.sln").to_str().unwrap(),
        "--configuration",
        "Debug",
        "--platform",
        "x64",
        "--output",
        temp.path().join("compile_commands.json").to_str().unwrap(),
    ]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("no Visual C++ projects"));
}

#[test]
fn slnx_applies_platform_mappings_and_build_rules() {
    let temp = TempDir::new().unwrap();
    let output = temp.path().join("compile_commands.json");
    let result = run(&[
        fixture("mapped.slnx").to_str().unwrap(),
        "--configuration",
        "Release",
        "--platform",
        "AnyCPU",
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let database = read_database(&output);
    assert_eq!(database.len(), 5);
    for entry in database {
        assert!(
            entry["arguments"]
                .as_array()
                .unwrap()
                .iter()
                .any(|argument| {
                    argument
                        .as_str()
                        .is_some_and(|value| value == "MS2CC_MAPPED_PLATFORM=1")
                })
        );
    }

    let result = run(&[
        fixture("mapped.slnx").to_str().unwrap(),
        "--configuration",
        "Debug",
        "--platform",
        "AnyCPU",
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("no Visual C++ projects"));
}

#[test]
fn successful_run_replaces_existing_database() {
    let temp = TempDir::new().unwrap();
    let output = temp.path().join("compile_commands.json");
    fs::write(
        &output,
        r#"[{"directory":"C:\\stale","arguments":["cl.exe","stale.cpp"],"file":"C:\\stale\\stale.cpp"}]"#,
    )
    .unwrap();

    let result = run(&[
        fixture("basic\\basic.vcxproj").to_str().unwrap(),
        "--configuration",
        "Release",
        "--platform",
        "x64",
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let database = read_database(&output);
    assert_eq!(database.len(), 5);
    assert!(
        !database
            .iter()
            .any(|entry| entry["file"] == "C:\\stale\\stale.cpp")
    );
}

#[test]
fn default_output_is_written_in_the_current_directory() {
    let temp = TempDir::new().unwrap();
    let result = run_in(
        temp.path(),
        &[
            fixture("basic\\basic.vcxproj").to_str().unwrap(),
            "--configuration",
            "Release",
            "--platform",
            "x64",
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        read_database(&temp.path().join("compile_commands.json")).len(),
        5
    );
}

#[test]
fn project_and_output_paths_may_contain_spaces() {
    let temp = TempDir::new().unwrap();
    let project_root = temp.path().join("project with spaces");
    let output_root = temp.path().join("output with spaces");
    copy_native_fixture(&project_root);
    fs::create_dir(&output_root).unwrap();
    let output = output_root.join("compile commands.json");
    let result = run(&[
        project_root.join("basic\\basic.vcxproj").to_str().unwrap(),
        "--configuration",
        "Release",
        "--platform",
        "x64",
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(read_database(&output).len(), 5);
}

#[test]
fn failure_preserves_existing_database() {
    let temp = TempDir::new().unwrap();
    let output = temp.path().join("compile_commands.json");
    let sentinel = b"existing database";
    fs::write(&output, sentinel).unwrap();

    let result = run(&[
        temp.path().join("missing.vcxproj").to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(!result.status.success());
    assert_eq!(fs::read(output).unwrap(), sentinel);
}

#[test]
fn extraction_failure_preserves_existing_database() {
    let temp = TempDir::new().unwrap();
    let output = temp.path().join("compile_commands.json");
    let sentinel = b"existing database";
    fs::write(&output, sentinel).unwrap();

    let result = run(&[
        fixture("broken.proj").to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(!result.status.success());
    assert_eq!(fs::read(output).unwrap(), sentinel);
}

#[test]
fn text_logs_are_rejected() {
    let temp = TempDir::new().unwrap();
    let input = temp.path().join("msbuild.log");
    let output = temp.path().join("compile_commands.json");
    fs::write(&input, "CL.exe /c stale.cpp").unwrap();

    let result = run(&[
        input.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("unsupported input"));
    assert!(!output.exists());
}

#[test]
fn conflicting_properties_are_rejected() {
    let result = run(&[
        fixture("basic\\basic.vcxproj").to_str().unwrap(),
        "--configuration",
        "Release",
        "--property",
        "configuration=Debug",
    ]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("specified more than once"));

    let result = run(&[
        fixture("basic\\basic.vcxproj").to_str().unwrap(),
        "--property",
        "DesignTimeBuild=false",
    ]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("reserved by ms2cc"));
}

#[test]
fn output_cannot_overwrite_an_input() {
    let project = fixture("basic\\basic.vcxproj");
    let result = run(&[
        project.to_str().unwrap(),
        "--output",
        project.to_str().unwrap(),
    ]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("cannot overwrite an input"));
}

#[test]
fn output_cannot_overwrite_discovered_projects_or_sources() {
    let temp = TempDir::new().unwrap();
    copy_native_fixture(temp.path());
    let root = temp.path().join("traversal.proj");

    for (relative, expected_error) in [
        ("nested.proj", "discovered project"),
        ("basic\\basic.vcxproj", "discovered project"),
        ("basic\\one.cpp", "source file"),
    ] {
        let output = temp.path().join(relative);
        let original = fs::read(&output).unwrap();
        let result = run(&[
            root.to_str().unwrap(),
            "--configuration",
            "Release",
            "--platform",
            "x64",
            "--output",
            output.to_str().unwrap(),
        ]);
        assert!(!result.status.success(), "{relative} should be protected");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(expected_error),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(fs::read(output).unwrap(), original);
    }
}

#[test]
fn output_cannot_overwrite_a_build_disabled_solution_member() {
    let temp = TempDir::new().unwrap();
    copy_reference_fixture(temp.path());
    let output = temp.path().join("excluded\\excluded.vcxproj");
    let original = fs::read(&output).unwrap();
    let result = run(&[
        temp.path().join("references.sln").to_str().unwrap(),
        "--configuration",
        "Release",
        "--platform",
        "x64",
        "--output",
        output.to_str().unwrap(),
    ]);

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("discovered project"));
    assert_eq!(fs::read(output).unwrap(), original);
}

#[test]
fn removed_legacy_flags_are_rejected() {
    for flag in [
        "--input-file",
        "--log-level",
        "--pretty-print",
        "--no-progress",
        "--overwrite",
        "--merge",
        "--allow-empty",
        "--allow-partial",
        "--msbuild-arg",
        "--profile",
    ] {
        let result = run(&[fixture("basic\\basic.vcxproj").to_str().unwrap(), flag]);
        assert!(!result.status.success(), "{flag} should be rejected");
    }
}

#[test]
fn timeout_stops_a_hung_msbuild_host() {
    let started = Instant::now();
    let result = run(&[
        fixture("basic\\basic.vcxproj").to_str().unwrap(),
        "--msbuild",
        fixture("slow-msbuild.cmd").to_str().unwrap(),
        "--timeout",
        "1",
    ]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("timeout"));
    assert!(started.elapsed() < Duration::from_secs(8));
}

#[test]
fn command_wrapper_paths_quote_cmd_metacharacters() {
    let temp = TempDir::new().unwrap();
    let wrapper_directory = temp.path().join("host & wrapper");
    fs::create_dir(&wrapper_directory).unwrap();
    let wrapper = wrapper_directory.join("msbuild.cmd");
    fs::copy(fixture("slow-msbuild.cmd"), &wrapper).unwrap();

    let result = run(&[
        fixture("basic\\basic.vcxproj").to_str().unwrap(),
        "--msbuild",
        wrapper.to_str().unwrap(),
        "--timeout",
        "1",
    ]);
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("timeout"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn clangd_accepts_preserved_msvc_pch_and_output_flags_when_available() {
    if Command::new("clangd").arg("--version").output().is_err() {
        return;
    }

    let temp = TempDir::new().unwrap();
    let output = temp.path().join("compile_commands.json");
    let result = run(&[
        fixture("basic\\basic.vcxproj").to_str().unwrap(),
        "--configuration",
        "Release",
        "--platform",
        "x64",
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    let result = Command::new("clangd")
        .arg(format!("--check={}", fixture("basic\\one.cpp").display()))
        .arg(format!("--compile-commands-dir={}", temp.path().display()))
        .arg("--background-index=0")
        .arg("--log=error")
        .output()
        .expect("failed to run clangd");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn materializes_msvc_environment_arguments() {
    let temp = TempDir::new().unwrap();
    let include = temp.path().join("environment include");
    let external_include = temp.path().join("external environment include");
    fs::create_dir(&include).unwrap();
    fs::create_dir(&external_include).unwrap();
    let output = temp.path().join("compile_commands.json");
    let result = Command::new(binary())
        .args([
            fixture("basic\\basic.vcxproj").to_str().unwrap(),
            "--configuration",
            "Release",
            "--platform",
            "x64",
            "--property",
            "UseEnv=true",
            "--output",
            output.to_str().unwrap(),
        ])
        .env("CL", "/DMS2CC_LEADING=1")
        .env("_CL_", "/DMS2CC_TRAILING=1")
        .env("INCLUDE", include.as_os_str())
        .env("EXTERNAL_INCLUDE", external_include.as_os_str())
        .output()
        .expect("failed to run ms2cc");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    for entry in read_database(&output) {
        let arguments: Vec<_> = entry["arguments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|argument| argument.as_str().unwrap())
            .collect();
        assert_eq!(
            arguments
                .iter()
                .filter(|argument| argument.contains("MS2CC_LEADING=1"))
                .count(),
            1
        );
        assert_eq!(
            arguments
                .iter()
                .filter(|argument| argument.contains("MS2CC_TRAILING=1"))
                .count(),
            1
        );
        assert!(arguments.iter().any(|argument| {
            argument
                .strip_prefix("/I")
                .is_some_and(|path| path.eq_ignore_ascii_case(include.to_str().unwrap()))
        }));
        assert!(arguments.iter().any(|argument| {
            argument
                .strip_prefix("/external:I")
                .is_some_and(|path| path.eq_ignore_ascii_case(external_include.to_str().unwrap()))
        }));
    }
}
