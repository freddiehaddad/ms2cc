use assert_cmd::prelude::*;
use serde_json::Value;
use std::fs::{self, File};
use std::io::Write;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn cli_generates_compile_commands() {
    let temp_dir = TempDir::new().expect("create temp dir");

    let source_dir = temp_dir.path().join("src");
    fs::create_dir_all(&source_dir).expect("create source dir");
    File::create(source_dir.join("main.cpp")).expect("touch source file");

    let log_path = temp_dir.path().join("msbuild.log");
    {
        let mut handle = File::create(&log_path).expect("create log file");
        writeln!(handle, "cl.exe /c main.cpp").expect("write log line");
    }

    let output_path = temp_dir.path().join("compile_commands.json");

    let mut cmd = Command::cargo_bin("ms2cc").expect("find binary");
    cmd.arg("--input-file")
        .arg(&log_path)
        .arg("--output-file")
        .arg(&output_path)
        .arg("--source-directory")
        .arg(&source_dir)
        .arg("--max-threads")
        .arg("1");

    cmd.assert().success();

    let contents = fs::read_to_string(&output_path).expect("read output");
    let json: Value = serde_json::from_str(&contents).expect("parse JSON");
    let commands = json.as_array().expect("array of commands");
    assert_eq!(commands.len(), 1);

    let entry = &commands[0];
    assert_eq!(entry["file"].as_str().unwrap(), "main.cpp");

    let expected_directory = source_dir.to_string_lossy().to_lowercase();
    assert_eq!(entry["directory"].as_str().unwrap(), expected_directory);

    let arguments = entry["arguments"].as_array().expect("arguments array");
    let values: Vec<&str> = arguments
        .iter()
        .map(|value| value.as_str().expect("string argument"))
        .collect();
    assert!(values.contains(&"cl.exe"));
    assert!(values.contains(&"main.cpp"));
}
