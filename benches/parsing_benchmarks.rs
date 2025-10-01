// benches/parsing_benchmarks.rs - Performance benchmarks for ms2cc

use criterion::{Criterion, criterion_group, criterion_main};
use ms2cc::{Config, parser};
use std::hint::black_box;

fn bench_ends_with_cpp_source_file(c: &mut Criterion) {
    let config = Config::default();
    let test_lines = vec![
        "cl.exe /c /Zi /nologo main.cpp",
        "cl.exe /c /Zi /nologo header.h",
        "cl.exe /c /Zi /nologo utils.cxx",
        "cl.exe /c /Zi /nologo \"quoted_file.cpp\"",
        "cl.exe /c /Zi /nologo file_without_extension",
        "very long compile command with many arguments /DWIN32 /D_WINDOWS /W3 /GR /EHsc /bigobj source.cpp",
    ];

    c.bench_function("ends_with_cpp_source_file", |b| {
        b.iter(|| {
            for line in &test_lines {
                black_box(parser::ends_with_cpp_source_file(
                    black_box(line),
                    black_box(&config.file_extensions),
                ));
            }
        })
    });
}

fn bench_should_exclude_directory(c: &mut Criterion) {
    let config = Config::default();
    let test_dirs = vec![
        ".git",
        "src",
        "include",
        "target",
        "build",
        ".vscode",
        "node_modules",
        "vendor",
    ];

    c.bench_function("should_exclude_directory", |b| {
        b.iter(|| {
            for dir in &test_dirs {
                black_box(parser::should_exclude_directory(
                    black_box(dir),
                    black_box(&config.exclude_directories),
                ));
            }
        })
    });
}

fn bench_should_process_file_extension(c: &mut Criterion) {
    let config = Config::default();
    let test_extensions = vec![
        "cpp", "c", "h", "hpp", "cxx", "cc", "txt", "rs", "py", "js", "java",
    ];

    c.bench_function("should_process_file_extension", |b| {
        b.iter(|| {
            for ext in &test_extensions {
                black_box(parser::should_process_file_extension(
                    black_box(ext),
                    black_box(&config.file_extensions),
                ));
            }
        })
    });
}

fn bench_tokenize_compile_command(c: &mut Criterion) {
    let test_commands = vec![
        "cl.exe /c main.cpp",
        "cl.exe /c /Zi /nologo /W3 /GR /EHsc /bigobj /DWIN32 /D_WINDOWS main.cpp",
        r#""C:\Program Files\cl.exe" /c /I"C:\Path With Spaces" "source file.cpp""#,
        r#"cl.exe "/D\"VALUE\"" "" "C:\path with spaces\file.cpp""#,
    ];

    c.bench_function("tokenize_compile_command", |b| {
        b.iter(|| {
            for cmd in &test_commands {
                black_box(parser::tokenize_compile_command(black_box(cmd)));
            }
        })
    });
}

criterion_group!(
    benches,
    bench_ends_with_cpp_source_file,
    bench_should_exclude_directory,
    bench_should_process_file_extension,
    bench_tokenize_compile_command
);
criterion_main!(benches);
