# MS2CC - MSBuild to Compile Commands

MS2CC is a Rust CLI that turns `msbuild.log` files into a `compile_commands.json` compliant [^1] database for C and C++ language servers. Point it at the build log from a full MSBuild build and you get an IDE-ready compilation database in seconds.

## Highlights

- Built for clangd, the Microsoft C/C++ extension, and other LSP clients that consume `compile_commands.json`.
- Iterator-driven pipeline that keeps memory use low while scanning logs and indexing source trees.
- Friendly CLI with sensible defaults for exclusions, extensions, and thread counts.

## Introduction

MS2CC enables full IDE-like support for C/C++ Language Server Protocol ([LSP]) clients. It ingests `msbuild.log` output from a complete MSBuild build and restructures each compile invocation into a `compile_commands.json` entry that language servers can understand. The database works seamlessly with tooling such as [clangd] and the [Microsoft C/C++ Extension] for Visual Studio Code.

The `compile_commands.json` file is a database that lists all source files in your project along with the exact commands needed to compile each one. Each entry follows this structure:

```json
[
    {
        "file": "main.cpp",
        "directory": "C:\\projects\\example",
        "arguments": ["cl.exe", "/EHsc", "/Zi", "/D", "DEBUG", "main.cpp"]
    }
]
```

## Installation

Grab a prebuilt release from GitHub or build the binary locally with the Rust toolchain.

### Building from Source

Since MS2CC is written in [Rust], install the toolchain first:

1. Follow the [Rust installation page] to set up `rustup`.
1. Clone the repository.
1. Run `cargo build` for a debug build or `cargo build --release` for an optimized binary.
1. Pick up the resulting executable from `target/debug/` or `target/release/`.

## Generating the Database

1. Complete a full build of your C/C++ MSBuild project so the `msbuild.log` contains every compile command.
1. Run `ms2cc.exe` (the default output is `compile_commands.json`; override it with `--output-file` if you prefer another path). Use `ms2cc.exe --help` to review every flag:

   ```console
   ms2cc.exe --input-file <PATH_TO_MSBUILD.LOG> --source-directory <PATH_TO_PROJECT_SOURCE>
   ```

```console
$ ms2cc.exe --help
Tool to generate a compile_commands.json database from an msbuild.log file.

Usage: ms2cc.exe [OPTIONS] --input-file <INPUT_FILE> --source-directory <SOURCE_DIRECTORY>

Options:
  -i, --input-file <INPUT_FILE>
          Path to msbuild.log
  -o, --output-file <OUTPUT_FILE>
          Output JSON file [default: compile_commands.json]
  -p, --pretty-print
          Pretty print output JSON file
  -d, --source-directory <SOURCE_DIRECTORY>
          Path to source code
  -x, --exclude-directories <EXCLUDE_DIRECTORIES>
          Directories to exclude during traversal (comma-separated) [default: .git]
  -e, --file-extensions <FILE_EXTENSIONS>
          File extensions to process (comma-separated) [default: c cc cpp cxx c++ h hh hpp hxx h++ inl]
  -c, --compiler-executable <EXE>
          Name of compiler executable [default: cl.exe]
  -t, --max-threads <MAX_THREADS>
          Max number of threads per task [default: 8]
  -h, --help
          Print help
  -V, --version
          Print version
```

### When to Regenerate the Database

Regenerate the database whenever you add, delete, move, or reconfigure source files so the language server stays in sync.

## Editor Configuration

To hook MS2CC into your editor:

1. Install a C/C++ language server.
1. Configure your editor of choice.
1. Point the language server at the generated database.

### Visual Studio Code

In VSCode, install the [Microsoft C/C++ Extension] to enable language support. It creates a `.vscode/c_cpp_properties.json` file that stores per-project settings. For each configuration, add a `compileCommands` entry like this:

```json
{
    "configurations": [
        {
            "compileCommands": "${workspaceFolder}/.vscode/compile_commands.json"
        }
    ]
}
```

> **NOTE:** Update the path to match the location of your `compile_commands.json` file.

### Other Editors

For most other editors, [clangd] is a great companion. It automatically walks up the directory tree (including `build/` subdirectories) to locate a `compile_commands.json`. For example, editing `$SRC/gui/window.cpp` prompts clangd to check `$SRC/gui/`, `$SRC/gui/build/`, `$SRC/`, `$SRC/build/`, and so on. You can also pin the path explicitly in your clangd configuration:

```yaml
CompileFlags:
  # Directory to compile_commands.json (omit the file name)
  CompilationDatabase: c:\\path\\to\\your\\compile_commands.json
```

> **TIP:** The [clangd configuration] documentation covers additional options.

## Validation & Troubleshooting

### Automated checks

```powershell
cargo fmt
cargo test
cargo bench
```

### Quick smoke test

Run a quick smoke test against a representative `msbuild.log` to confirm end-to-end parsing. This command writes the database to `compile_commands.json` and pretty-prints the result:

```powershell
cargo run -- --input-file path\to\msbuild.log --source-directory path\to\src --pretty-print
```

## Known Issues

### Handling Duplicate Source File Names with Relative Paths

Projects that reuse file names can confuse MSBuild logs when the compiler omits absolute paths. MS2CC tries to disambiguate entries with the `/Fo` flag, but if the emitted object file lives outside the source directory the lookup may still fail. In those cases MS2CC reports a descriptive error so you can adjust the build.

## Contributing

Contributions are welcome. Simply open a PR!

## Contact

For questions or comments, start a [discussion on GitHub].

[^1]: <https://clang.llvm.org/docs/JSONCompilationDatabase.html>

[clangd]: https://clangd.llvm.org/
[clangd configuration]: https://clangd.llvm.org/config
[discussion on GitHub]: https://github.com/fhaddad_microsoft/ms2cc/discussions
[lsp]: https://microsoft.github.io/language-server-protocol/
[microsoft c/c++ extension]: https://code.visualstudio.com/docs/languages/cpp
[rust]: https://www.rust-lang.org/
[rust installation page]: https://www.rust-lang.org/tools/install
