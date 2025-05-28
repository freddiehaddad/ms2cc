# MS2CC - MSBuild to Compile Commands

A Rust CLI tool that uses `msbuild.log` files to generate a
`compile_commands.json` compliant [^1] database for use with C/C++ language
servers.

## Introduction

This tool enables full IDE-like functionality for C/C++ Language Server Protocol
([LSP]) compliant language servers that rely on a `compile_commands.json`
database. It works by converting `msbuild.log` output files, generated when
performing a full MSBuild project build, into a `compile_commands.json`
database. Two such language servers are [clangd] and [Microsoft C/C++ Extension]
for Visual Studio Code.

The `compile_commands.json` file is a database that lists all source files in
your project along with the exact commands needed to compile each one. Each
entry follows this structure:

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

There are two ways to obtain the `ms2cc.exe` binary: download a precompiled
release or build it from source.

### Building from Source

Since the tool was written in [Rust], the Rust development toolchain is required
for compilation.

1. Prepare your environment by following the instructions on the
   [Rust installation page].
1. Clone the repository.
1. Build a debug release with `cargo build` or a release version with
   `cargo build --release`.
1. The generated executable will be under the `./target/debug/` or
   `./target/release/` directory.

## Generating the Database

1. Complete a full build of your C/C++ MSBuild project.

1. Run `ms2cc.exe`:

   ```console
   ms2cc.exe --input-file <PATH_TO_MSBUILD.LOG> --source-directory <PATH_TO_PROJECT_SOURCE>
   ```

   This will create a `compile_commands.json` file in the same directory where
   you ran `ms2cc.exe`.

   You can use the `--output-file` option to specify the name and location for
   the database to be written.

> **NOTE:** You can run `ms2cc.exe` with the `--help` option for more details.

```console
$ ms2cc.exe --help
Tool to generate a compile_commands.json file from an msbuild.log file.

Usage: ms2cc.exe [OPTIONS] --input-file <INPUT_FILE> --source-directory <SOURCE_DIRECTORY>

Options:
  -i, --input-file <INPUT_FILE>              Path to msbuild.log
  -o, --output-file <OUTPUT_FILE>            Output JSON file [default: compile_commands.json]
  -d, --source-directory <SOURCE_DIRECTORY>  Path to source code
  -c, --compiler-executable <EXE>            Name of compiler executable [default: cl.exe]
  -h, --help                                 Print help
  -V, --version                              Print version
```

### When to Regenerate the Database

Several conditions require regenerating the `compile_commands.json` database:

1. Source files are added to, removed from, or relocated within the project.
1. Changes are made to any source file's compile commands.

## Editor Configuration

Configuring your editor to make use of the `compile_commands.json` database
requires a few steps:

1. Install a C/C++ language server
1. Configure your editor
1. Point the language server to your project

### Visual Studio Code

In VSCode, you can install the [Microsoft C/C++ Extension] to enable C/C++
language support. Once installed, a `.vscode` directory will be created in your
project folder, containing a `c_cpp_properties.json` file. The `configurations`
key in this file holds an array of project-specific settings. For each relevant
configuration, add the `compileCommands` property as shown below:

```json
{
    "configurations": [
        {
            "compileCommands": "${workspaceFolder}/.vscode/compile_commands.json"
        }
    ]
}
```

> **NOTE:** Set the path to the location of your `compile_commands.json` file.

### Other Editors

For most editors, [clangd] is likely the best choice. It automatically searches
for a `compile_commands.json` file by traversing up the directory tree from the
file you're editing, as well as checking any `build/` subdirectories. For
instance, if you're editing `$SRC/gui/window.cpp,` clangd will look in
`$SRC/gui/`, `$SRC/gui/build/`, `$SRC/`, `$SRC/build/`, and so on. You can also
explicitly specify the path to the compilation database in your clangd
configuration file:

```yaml
CompileFlags:
  CompilationDatabase: c:\\path\\to\\your\\compile_commands.json
```

> **NOTE:** See the [clangd configuration] documentation for details.

## Known Issues

**Handling Duplicate Source File Names with Relative Paths**

In some cases, multiple source files in a project may share the same name, and
the associated compile commands might not include absolute file paths. Since
absolute paths are required, the tool attempts to disambiguate these entries
using the `/Fo` compiler flag (if present in the compile command), which
specifies the output object file. However, if the object file path lies outside
the directory of the corresponding source file, this method may fail. When that
happens, an error message is logged.

## Contributing

Contributions are welcome. Simply open a PR!

## Contact

For questions or comments, please start a [discussion on github].

[^1]: https://clang.llvm.org/docs/JSONCompilationDatabase.html

[clangd]: https://clangd.llvm.org/
[clangd configuration]: https://clangd.llvm.org/config
[discussion on github]: https://github.com/fhaddad_microsoft/ms2cc/discussions
[lsp]: https://microsoft.github.io/language-server-protocol/
[microsoft c/c++ extension]: https://code.visualstudio.com/docs/languages/cpp
[rust]: https://www.rust-lang.org/
[rust installation page]: https://www.rust-lang.org/tools/install
