# ms2cc

> Convert MSBuild logs to compile_commands.json for C/C++ language servers

## Table of Contents

- [What is ms2cc?](#what-is-ms2cc)
- [Why is ms2cc useful?](#why-is-ms2cc-useful)
- [Requirements](#requirements)
- [Installation](#installation)
- [Quick Start](#quick-start)
- [Usage](#usage)
- [Editor Configuration](#editor-configuration)
  - [Visual Studio Code](#visual-studio-code)
  - [Other Editors](#other-editors)
- [Troubleshooting](#troubleshooting)
- [LSP and AI: Better Together](#lsp-and-ai-better-together)
- [References](#references)
- [License](#license)

## What is ms2cc?

ms2cc converts MSBuild build logs into [`compile_commands.json`][compile-commands] format. This compilation database is used by [language servers][lsp], and various IDEs for code intelligence features (IntelliSense, navigation, refactoring, linting).

[MSBuild][msbuild-cli] does not generate [`compile_commands.json`][compile-commands]. ms2cc extracts compiler invocations from MSBuild's detailed logs and converts them to the standard format.

## Why is ms2cc Useful?

C and C++ [language servers][lsp] (like [clangd][clangd], ccls, Microsoft C/C++, etc.) need a [`compile_commands.json`][compile-commands] file because they cannot correctly understand or parse your code without the exact compiler flags that are used when building the project.

Unlike languages such as Rust, Go, Python, or Java, C and C++ code is not self-describing. A C/C++ source file:

- depends on preprocessor macros
- depends on include paths (-I)
- depends on system headers
- can change behavior based on platform and compiler
- may use language extensions (-std=gnu++20, -fms-extensions)
- may be compiled differently per file

This means the same `.cpp` file can produce totally different ASTs depending on its flags.

A language server must replicate the exact compiler invocation, or it literally cannot parse the file in the correct configuration.

## Requirements

- **Platform:** Windows (Windows 10/11, Windows Server)
- **Build System:** MSBuild (Visual Studio 2019, 2022, or Build Tools)
- **For Building from Source (optional):** Rust toolchain

## Installation

### Option A: Download Pre-built Binary

1. **Download:**
   - Visit the [releases page](https://github.com/freddiehaddad/ms2cc/releases)
   - Download the `ms2cc-x.x.x.zip` file

2. **Extract:**
   - Unzip the file to extract `ms2cc.exe`

3. **Usage:**
   - Run it from anywhere by adding the directory to your PATH, or
   - Copy `ms2cc.exe` to your project directory

### Option B: Build from Source

1. **Install [Rust][rust]** (if not already installed):

   ```text
   # Visit https://rustup.rs and follow the instructions
   ```

2. **Clone the repository:**

   ```powershell
   git clone https://github.com/freddiehaddad/ms2cc.git
   cd ms2cc
   ```

3. **Build the release version:**

   ```powershell
   cargo build --release
   ```

4. **Find the executable:**

   ```text
   The built executable will be at: target\release\ms2cc.exe
   ```

## Quick Start

### Build with Detailed Logging

```powershell
cd C:\path\to\your\project
msbuild YourSolution.sln /v:detailed > msbuild.log
```

The `/v:detailed` flag is required. Without it, [MSBuild][msbuild-cli] doesn't log enough information.

> **Visual Studio IDE:** In Visual Studio 2019/2022 use **Build > Project Only > Build Only ProjectName**. When the Output window finishes scrolling, right-click inside it, choose [**Save Build Log**][vs-build-logging], and save it as `msbuild.log` with **MSBuild Project Build Log (\*.log)**.

### Generate compile_commands.json

```powershell
ms2cc -i msbuild.log -o compile_commands.json
```

### Configure Your Editor

See the [Editor Configuration](#editor-configuration) section below for your specific editor.

### Verify the Output

```powershell
Test-Path compile_commands.json
Get-Content compile_commands.json | Select-Object -First 8
```

The file should exist and contain one JSON object per compiler invocation, for example:

```json
[
  {
    "file": "src\\main.cpp",
    "directory": "C:/path/to/your/project",
    "arguments": ["cl.exe", "/Iinclude", "src/main.cpp"]
  }
]
```

If the file is missing or empty, revisit the logging step.

## Usage

### Command-Line Options

ms2cc supports several options for customizing behavior:

```powershell
# Basic usage (uses defaults: msbuild.log → compile_commands.json)
ms2cc

# Specify custom input and output paths
ms2cc -i build\debug.log -o compile_commands.json

# Pretty-print the JSON output (useful for viewing/debugging)
ms2cc -i msbuild.log -o compile_commands.json -p

# Quiet mode (only show errors)
ms2cc -i msbuild.log -o compile_commands.json -l error

# Disable progress bars (useful for scripting)
ms2cc -i msbuild.log -o compile_commands.json --no-progress

# Show all available options
ms2cc --help

# Show version
ms2cc --version
```

### Available Options

| Option                     | Description                                          | Default                 |
| -------------------------- | ---------------------------------------------------- | ----------------------- |
| `-i, --input-file <FILE>`  | Path to MSBuild log file                             | `msbuild.log`           |
| `-o, --output-file <FILE>` | Path to output compile_commands.json                 | `compile_commands.json` |
| `-l, --log-level <LEVEL>`  | Logging level (off, error, warn, info, debug, trace) | `info`                  |
| `-p, --pretty-print`       | Pretty-print JSON output                             | (disabled)              |
| `--no-progress`            | Disable progress bar output                          | (progress bars enabled) |
| `-h, --help`               | Display help information                             | -                       |
| `-V, --version`            | Display version information                          | -                       |

## Editor Configuration

Once you've generated `compile_commands.json`, configure your editor to use it.

> **Note:** The `compile_commands.json` file should be in your project root directory.

### Visual Studio Code

#### Using Microsoft C/C++ Extension

1. Install the extension
   1. Open Extensions (`Ctrl+Shift+X`)
   2. Search for "C/C++"
   3. Install the official [Microsoft C/C++ extension][ms-cpp-ext]

2. Configure

   Create or edit `.vscode/c_cpp_properties.json`:

   ```json
   {
     "configurations": [
       {
         "name": "Win32",
         "compileCommands": "${workspaceFolder}/compile_commands.json",
         "intelliSenseMode": "windows-msvc-x64"
       }
     ],
     "version": 4
   }
   ```

   > **Note:** You can place the `compile_commands.json` file in the `.vscode` directory if you prefer:

   ```json
   {
     "configurations": [
       {
         "name": "Win32",
         "compileCommands": "${workspaceFolder}/.vscode/compile_commands.json",
         "intelliSenseMode": "windows-msvc-x64"
       }
     ],
     "version": 4
   }
   ```

3. Reload VSCode

   Press `Ctrl+Shift+P`, type "Reload Window", and press Enter.

4. Confirm the setup by opening a C/C++ file, pressing `Ctrl+Shift+P`, running `C/C++: Log Diagnostics`, and making sure the output reports no parsing or IntelliSense errors.

#### Using clangd

1. Install the [clangd][clangd] extension
   1. Open VSCode Extensions (`Ctrl+Shift+X`)
   2. Search for "clangd"
   3. Install the official `llvm-vs-code-extensions.vscode-clangd` extension
   4. If prompted to download clangd, click "Download"

2. Disable Microsoft C++ IntelliSense

   Create or edit `.vscode/settings.json` in your project:

   ```json
   {
     "C_Cpp.intelliSenseEngine": "disabled",
     "clangd.arguments": [
       "--compile-commands-dir=${workspaceFolder}",
       "--background-index",
       "--clang-tidy"
     ]
   }
   ```

3. Reload VSCode

   Press `Ctrl+Shift+P`, type "Reload Window", and press Enter.

> **Note for MSVC Projects**: For optimal clangd compatibility with MSVC code, copy `.clangd.template` to `.clangd` in your project root. This configuration suppresses common MSVC-specific warnings. See [Using clangd with MSVC Projects](docs/CLANGD_USAGE.md) for details.

### Other Editors

Most modern editors have excellent clangd support. For detailed setup instructions including:

- **Neovim**
- **Zed**
- **Cursor**
- **Helix**
- Other LSP-compatible editors

See the **[Using clangd with MSVC Projects](docs/CLANGD_USAGE.md)** guide for complete configuration examples, compatibility information, and troubleshooting.

**Quick clangd setup:**

1. Generate `compile_commands.json` with ms2cc
2. (Optional but recommended) Copy `.clangd.template` to `.clangd` in your project root
3. Configure your editor to use clangd (see guide for editor-specific examples)

## Troubleshooting

### No compile_commands.json generated or file is empty

**Cause:** MSBuild log doesn't have enough detail.

**Solution:** Ensure you used `/v:detailed` when building:

```powershell
msbuild YourSolution.sln /v:detailed > msbuild.log
```

### Some source files are missing from compile_commands.json

**Possible causes:**

- MSBuild verbosity too low
- Some projects were skipped during build
- Incremental build didn't compile all files

**Solutions:**

1. Use `/v:detailed` verbosity
2. Do a clean rebuild: `msbuild YourSolution.sln /t:Rebuild /v:detailed > msbuild.log`
3. Delete any stale `compile_commands.json` files before regenerating, especially after switching configurations (Debug/Release, Win32/x64)

### Editor doesn't recognize compile_commands.json

**Solution:**

1. Ensure `compile_commands.json` is in your **project root directory**
2. Check your editor configuration (see [Editor Configuration](#editor-configuration))
3. For clangd, verify it's installed and accessible:

   ```powershell
   clangd --version
   ```

4. Reload/restart your editor

### Duplicate symbols or conflicting IntelliSense

**Cause:** Multiple IntelliSense engines running simultaneously.

**Solution (VSCode):** Disable Microsoft C++ IntelliSense when using clangd:

In `.vscode/settings.json`:

```json
{
  "C_Cpp.intelliSenseEngine": "disabled"
}
```

### clangd not found

**Solution:**

1. Download clangd from [https://github.com/clangd/clangd/releases](https://github.com/clangd/clangd/releases)
2. Add to your PATH, or configure the full path in your editor settings

### clangd shows too many warnings with MSVC code

**Cause:** clangd reports some MSVC-specific warnings that aren't relevant (e.g., `#pragma optimize`, missing field initializers).

**Solution:** Use the provided `.clangd` configuration template:

1. Copy `.clangd.template` to `.clangd` in your project root
2. Customize as needed (see comments in the template)
3. For detailed information, see [Using clangd with MSVC Projects](docs/CLANGD_USAGE.md)

The template suppresses common MSVC-specific warnings while keeping important diagnostics.

## LSP and AI: Better Together

With the rise of AI-powered coding assistants, some developers believe LSP is antiquated. However, LSP remains essential for high-quality code intelligence for several reasons:

1. They provide deterministic, real-time guarantees

   The [Language Server Protocol][lsp] gives editors fast, reliable, incremental features such as:
   - autocompletion
   - hover info and signature help
   - diagnostics in real time
   - semantic tokenization
   - symbol indexing
   - "go to definition" and cross-reference analysis

   These features work because LSP servers maintain an up-to-date AST, symbol tables, type information, and can react within milliseconds.

   LLM-based agents cannot match that determinism or sub-50 ms latency reliably, especially on larger codebases.

2. LSP understands project semantics in a ways LLMs cannot match

   Even with retrieval or agent-based navigation, LLMs still:
   - lack precise understanding of type systems
   - can hallucinate unseen functions or APIs
   - struggle with multi-file, incremental code state
   - cannot maintain a full AST-level view the way a compiler or language server can

   LSPs plug directly into the compiler toolchain; that precision is not replaceable by probabilistic models alone.

3. Agents build on top of LSP features -- not instead of them

   Modern coding assistants (GitHub Copilot, Cursor AI, Codeium, Windsurf, Zed AI, etc.) typically use:
   - LLM (reasoning + generation)
   - LSP (semantic signals, types, diagnostics)
   - Indexers (global symbol search, embeddings)

   LLMs work best when they are grounded in deterministic data -- and LSP is the grounding layer.

   Agents rely on:
   - LSP diagnostics to know what's broken
   - LSP symbol info to find definitions
   - LSP semantic tokens to reason about structure
   - LSP type information to provide accurate code suggestions

   If LSP didn’t exist, coding agents would be worse, not better.

That's why ms2cc exists -- to ensure your C/C++ projects have the LSP foundation that makes both manual coding and AI assistance better. Run ms2cc whenever you regenerate build logs so `compile_commands.json` stays fresh as projects, configurations, or toolchains change.

## References

- [Language Server Protocol (LSP)][lsp]
- [clangd - C/C++ Language Server][clangd]
- [clangd Configuration][clangd-config]
- [Using clangd with MSVC Projects](docs/CLANGD_USAGE.md)
- [`compile_commands.json` Format][compile-commands]
- [Microsoft C/C++ Extension for VSCode][ms-cpp-ext]
- [MSBuild Command-Line Reference][msbuild-cli]
- [Visual Studio Build Logging][vs-build-logging]
- [Rust Programming Language][rust]

[lsp]: https://microsoft.github.io/language-server-protocol/
[clangd]: https://clangd.llvm.org/
[clangd-config]: https://clangd.llvm.org/config.html
[compile-commands]: https://clang.llvm.org/docs/JSONCompilationDatabase.html
[ms-cpp-ext]: https://marketplace.visualstudio.com/items?itemName=ms-vscode.cpptools
[msbuild-cli]: https://learn.microsoft.com/en-us/visualstudio/msbuild/msbuild-command-line-reference
[vs-build-logging]: https://learn.microsoft.com/en-us/visualstudio/ide/build-log-file-visual-studio
[rust]: https://www.rust-lang.org/
[LICENSE]: LICENSE.txt

## License

This project is licensed under the MIT License - see the [LICENSE] file for details.
