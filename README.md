# ms2cc

> Convert MSBuild logs to compile_commands.json for C/C++ language servers

## Table of Contents

- [What is ms2cc?](#what-is-ms2cc)
- [Requirements](#requirements)
- [Installation](#installation)
- [Quick Start](#quick-start)
- [Usage](#usage)
- [Editor Configuration](#editor-configuration)
  - [Visual Studio Code](#visual-studio-code)
  - [Neovim](#neovim)
- [Troubleshooting](#troubleshooting)
- [LSP and AI: Better Together](#lsp-and-ai-better-together)
- [License](#license)

## What is ms2cc?

ms2cc converts MSBuild build logs into `compile_commands.json` format. This compilation database is used by clangd, clang-tidy, and various IDEs for code intelligence features (IntelliSense, navigation, refactoring, linting).

MSBuild does not generate `compile_commands.json`. ms2cc extracts compiler invocations from MSBuild's detailed logs and converts them to the standard format.

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

1. **Install Rust** (if not already installed):

   ```powershell
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
   ```
   The built executable will be at: target\release\ms2cc.exe
   ```

## Quick Start

### Build with Detailed Logging

```powershell
cd C:\path\to\your\project
msbuild YourSolution.sln /v:detailed > msbuild.log
```

The `/v:detailed` flag is required. Without it, MSBuild doesn't log enough information.

### Generate compile_commands.json

```powershell
ms2cc -i msbuild.log -o compile_commands.json
```

### Configure Your Editor

See the [Editor Configuration](#editor-configuration) section below for your specific editor.

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

# Disable progress bars (useful for CI/CD or scripting)
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

**Install the extension**

1. Open Extensions (`Ctrl+Shift+X`)
2. Search for "C/C++"
3. Install the official Microsoft C/C++ extension

**Configure compile_commands.json**

Create or edit `.vscode/c_cpp_properties.json`:

```json
{
  "configurations": [
    {
      "name": "Win32",
      "compileCommands": "${workspaceFolder}/compile_commands.json",
      "intelliSenseMode": "msvc-x64"
    }
  ],
  "version": 4
}
```

**Reload VSCode**

Press `Ctrl+Shift+P`, type "Reload Window", and press Enter.

#### Using clangd

**Install the clangd extension**

1. Open VSCode Extensions (`Ctrl+Shift+X`)
2. Search for "clangd"
3. Install the official `llvm-vs-code-extensions.vscode-clangd` extension
4. If prompted to download clangd, click "Download"

**Disable Microsoft C++ IntelliSense**

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

**Reload VSCode**

Press `Ctrl+Shift+P`, type "Reload Window", and press Enter.

### Neovim

**Install clangd**

- [clangd releases](https://github.com/clangd/clangd/releases)
- Add to your PATH or note the installation path

**Install nvim-lspconfig**

#### Using lazy.nvim

```lua
{
  "neovim/nvim-lspconfig",
  config = function()
    require("lspconfig").clangd.setup{
      cmd = {
        "clangd",
        "--background-index",
        "--clang-tidy",
        "--compile-commands-dir=.",
      },
      root_dir = require("lspconfig").util.root_pattern(
        "compile_commands.json",
        ".git"
      ),
    }
  end
}
```

#### Using Neovim's built-in package manager (Neovim 0.12+)

Clone nvim-lspconfig:

```powershell
git clone https://github.com/neovim/nvim-lspconfig.git "$env:LOCALAPPDATA\nvim-data\site\pack\plugins\start\nvim-lspconfig"
```

Add to your `init.lua`:

```lua
require'lspconfig'.clangd.setup{
  cmd = {
    "clangd",
    "--background-index",
    "--clang-tidy",
    "--compile-commands-dir=.",
  },
  root_dir = require'lspconfig'.util.root_pattern(
    "compile_commands.json",
    ".git"
  ),
}
```

**Place compile_commands.json in project root**

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

## LSP and AI: Better Together

With the rise of AI-powered coding assistants, some developers beleive LSP is antiquated. However, LSP remains essential for high-quality code intelligence for several reasons:

1. They provide deterministic, real-time guarantees

   The Language Server Protocol gives editors fast, reliable, incremental features such as:
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
   - LSP diagnostics to know what’s broken
   - LSP symbol info to find definitions
   - LSP semantic tokens to reason about structure
   - LSP type information to provide accurate code suggestions

   If LSP didn’t exist, coding agents would be worse, not better.

That's why ms2cc exists -- to ensure your C/C++ projects have the LSP foundation that makes both manual coding and AI assistance better.

## License

This project is licensed under the MIT License - see the [LICENSE.txt](LICENSE.txt) file for details.
