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
msbuild YourSolution.sln /fileLogger /fileLoggerParameters:LogFile=msbuild.log;Verbosity=detailed
```

The `Verbosity=detailed` parameter is required. Without it, [MSBuild][msbuild-cli] doesn't log enough information. We use MSBuild's built-in file logger (`/fileLogger /fileLoggerParameters:`) rather than PowerShell redirection (`> msbuild.log`) because Windows PowerShell 5.1 writes redirected output as UTF-16 LE, which ms2cc cannot read. The `/fileLogger` approach writes the file as UTF-8 regardless of shell. (Short forms `/fl` and `/flp:` work identically.)

> **Visual Studio IDE:** In Visual Studio 2019/2022 use **Build > Project Only > Build Only ProjectName**. When the Output window finishes scrolling, right-click inside it, choose [**Save Build Log**][vs-build-logging], and save it as `msbuild.log` with **MSBuild Project Build Log (\*.log)**.

### Generate compile_commands.json

```powershell
ms2cc -i msbuild.log -o compile_commands.json -p
```

The `-p` flag pretty-prints the output, which is useful for the verification step below. Omit it for production use to keep the file smaller.

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
    "directory": "C:\\path\\to\\your\\project",
    "command": "CL.exe /c /nologo /W4 /std:c++20 /Iinclude \"C:\\path\\to\\your\\project\\src\\main.cpp\"",
    "file": "C:\\path\\to\\your\\project\\src\\main.cpp"
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

# Overwrite mode (replace instead of merging with existing database)
ms2cc -i msbuild.log -o compile_commands.json --overwrite

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
| `--overwrite`              | Replace output file instead of merging               | (merge enabled)         |
| `--no-progress`            | Disable progress bar output                          | (progress bars enabled) |
| `-h, --help`               | Display help information                             | -                       |
| `-V, --version`            | Display version information                          | -                       |

### Incremental Builds

By default, ms2cc **merges** new entries into an existing `compile_commands.json` rather than replacing it. This means incremental builds work correctly — only the recompiled files are updated while entries for unchanged files are preserved.

Entries are matched by their source file path and project directory. If a file was recompiled, its entry is updated. If a file wasn't recompiled (and therefore not in the new build log), its existing entry is left untouched.

To start fresh and replace the entire database, use `--overwrite`:

```powershell
ms2cc -i msbuild.log -o compile_commands.json --overwrite
```

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

**Solution:** Ensure you used `Verbosity=detailed` when building:

```powershell
msbuild YourSolution.sln /fileLogger /fileLoggerParameters:LogFile=msbuild.log;Verbosity=detailed
```

### Some source files are missing from compile_commands.json

**Possible causes:**

- MSBuild verbosity too low
- Some projects were skipped during build

**Solutions:**

1. Use `Verbosity=detailed`
2. Do a clean rebuild and use `--overwrite`: `msbuild YourSolution.sln /t:Rebuild /fileLogger /fileLoggerParameters:LogFile=msbuild.log;Verbosity=detailed; ms2cc --overwrite`

> **Note:** With incremental builds, ms2cc merges new entries into the existing database by default. If you've done a full rebuild and want a clean database, use `--overwrite` to avoid retaining stale entries from a previous build configuration.

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

With the rise of AI-powered coding assistants, some developers wonder whether language-server tooling is still relevant. It is -- arguably more than before. The two solve different problems and work best in combination.

### Right tool for the job

| Task                                  | Best tool                                  |
| ------------------------------------- | ------------------------------------------ |
| "What's the type of this variable?"   | Language server                            |
| "Find every caller of this function"  | Language server                            |
| "Rename this symbol everywhere"       | Language server                            |
| "Show me errors as I type"            | Language server                            |
| "Did my last edit break anything?"    | Language server (diagnostics in ms)        |
| "Jump to this symbol's definition"    | Language server                            |
| "Write a function that does X"        | LLM                                        |
| "Why is this code buggy?"             | LLM (often)                                |
| "Explain this 500-line file"          | LLM                                        |
| "Generate boilerplate for this design"| LLM                                        |

Generation, exploration, and natural-language reasoning are LLM strengths. Symbol-precise queries, refactoring, and millisecond-latency feedback are language-server strengths. Neither replaces the other.

### Why language servers still matter

1. Deterministic, low-latency answers

   Language servers like [clangd][clangd] maintain an up-to-date AST, symbol table, and type information, and typically respond in 10-200 ms warm. LLMs are non-deterministic by design (sampling) and run 500 ms-5 s+. For typing flow, "go to definition", and refactor-rename, you want the correct answer every time, not a probabilistic best-guess.

2. Project-semantics precision

   LLMs reason over tokens, not syntax trees. Even with retrieval or agent-based navigation, they:
   - lack precise understanding of type systems (templates, overload resolution, SFINAE)
   - can hallucinate unseen functions or APIs
   - struggle with multi-file, incremental code state
   - cannot maintain a full AST-level view the way a compiler or language server can

   Language servers plug directly into the compiler toolchain; that precision isn't replaceable by probabilistic models alone.

3. Cost, privacy, and offline

   Language servers run locally and free. LLM agents cost money per token and typically send code to a remote server. For interactive feedback (squiggles, hover, completion) you want a local tool.

### Agents build *on* LSP, not instead of it

Modern coding assistants -- GitHub Copilot, Cursor, Windsurf, Zed AI, Cline, Aider, Continue -- combine three layers:

- **LLM** for reasoning and generation
- **Language servers** for semantic signals (types, diagnostics, symbol info, references)
- **Indexers** for global symbol search and embeddings

The LLM is the loud part; the language server is the grounding part. When a chat-style assistant answers "what's the type of `foo`?", the editor passes LSP-derived type info into the prompt -- the model doesn't guess. And the agentic "edit -> check diagnostics -> fix" inner loop that makes coding agents tractable is built on LSP: without millisecond-latency diagnostics, the loop becomes "edit -> run full build -> parse stderr -> fix" with seconds-to-minutes per iteration.

Concrete evidence that agents lean on LSP:

- **Cursor's Composer** consumes LSP signals heavily; disabling the language server visibly degrades edit quality.
- **Aider** built its own structural index (a Tree-sitter "repo-map") for setups where LSP wasn't reliable -- they needed structural signals badly enough to reinvent a subset of LSP themselves.
- The **[Model Context Protocol (MCP)][mcp]** ecosystem ships multiple LSP-as-MCP-server implementations precisely so agents can call language servers directly as tools.
- Agentic CLIs (Claude Code, Codex CLI, GitHub Copilot CLI, Cline) all leverage LSP via the editor or a wrapper when available.

If LSP -- or any comparable source of structural code intelligence -- didn't exist, coding agents would be worse, not better. This isn't theoretical; it's observable in every serious agent shipping today.

### Why this matters more in an AI-heavy workflow

Coding agents write code faster than humans can hand-check it. Catching their mistakes -- type errors, broken refactors, calls to APIs that don't exist -- requires fast, accurate semantic feedback as you read and review the generated code. That feedback comes from a language server reading `compile_commands.json`. Without it, you're relying on the LLM to have been correct in the first place, which is exactly the failure mode the language server is there to catch.

That's why ms2cc exists -- to give your C/C++ projects the language-server foundation that makes both manual coding and AI assistance dramatically more effective. Run ms2cc whenever you regenerate build logs so `compile_commands.json` stays fresh as projects, configurations, or toolchains change.

## References

- [Language Server Protocol (LSP)][lsp]
- [clangd - C/C++ Language Server][clangd]
- [clangd Configuration][clangd-config]
- [Using clangd with MSVC Projects](docs/CLANGD_USAGE.md)
- [`compile_commands.json` Format][compile-commands]
- [Microsoft C/C++ Extension for VSCode][ms-cpp-ext]
- [MSBuild Command-Line Reference][msbuild-cli]
- [Visual Studio Build Logging][vs-build-logging]
- [Model Context Protocol (MCP)][mcp]
- [Rust Programming Language][rust]

[lsp]: https://microsoft.github.io/language-server-protocol/
[clangd]: https://clangd.llvm.org/
[clangd-config]: https://clangd.llvm.org/config.html
[compile-commands]: https://clang.llvm.org/docs/JSONCompilationDatabase.html
[ms-cpp-ext]: https://marketplace.visualstudio.com/items?itemName=ms-vscode.cpptools
[msbuild-cli]: https://learn.microsoft.com/en-us/visualstudio/msbuild/msbuild-command-line-reference
[vs-build-logging]: https://learn.microsoft.com/en-us/visualstudio/ide/build-log-file-visual-studio
[rust]: https://www.rust-lang.org/
[mcp]: https://modelcontextprotocol.io/
[LICENSE]: LICENSE.txt

## License

This project is licensed under the MIT License - see the [LICENSE] file for details.
