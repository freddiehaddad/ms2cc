# ms2cc

`ms2cc` creates a
[`compile_commands.json`](https://clang.llvm.org/docs/JSONCompilationDatabase.html)
file for C and C++ projects built with Visual Studio and MSBuild.

Tools such as clangd and clang-tidy use this file to find your source files and
understand the same include paths, preprocessor definitions, and compiler
options used by your build.

## Requirements

- Windows
- Visual Studio 2022 or Build Tools 2022, version 17.8 or newer
- The **Desktop development with C++** workload
- Any packages and generated files required by your project

Run `ms2cc` from a command prompt where your project already builds correctly.
If your repository provides a setup script, run that script first.

## Install

Download the Windows executable from the
[releases page](https://github.com/freddiehaddad/ms2cc/releases), or build it
from source:

```powershell
cargo build --release
```

The executable is written to `target\release\ms2cc.exe`.

Place `ms2cc.exe` in a directory listed in `PATH`, or invoke it using its full
path.

## Quick start

Open a Visual Studio Developer Command Prompt, change to your repository, and
pass `ms2cc` your solution or project:

```powershell
cd C:\path\to\repository
ms2cc .\Project.sln
```

This creates `compile_commands.json` in the current directory.

For a large repository with a top-level `.proj` file, pass only that file.
`ms2cc` follows project references automatically:

```powershell
ms2cc .\Build.proj
```

You do not need to list every project below it.

Configuration and platform are optional. `ms2cc` uses the values provided by
MSBuild and the project. If the solution has more than one possible choice and
no clear default, choose one explicitly:

```powershell
ms2cc .\Project.sln --configuration Debug --platform x64
```

## More examples

Write the database somewhere else:

```powershell
ms2cc .\Project.sln --output C:\work\compile_commands.json
```

Combine independent solutions or projects into one database:

```powershell
ms2cc .\Engine.sln .\Tools.sln --output .\compile_commands.json
```

Pass a property required by your build:

```powershell
ms2cc .\Build.proj --property BuildFlavor=Production
```

`--property` may be repeated.

Use a specific MSBuild installation:

```powershell
ms2cc .\Project.sln `
  --msbuild C:\BuildTools\MSBuild\Current\Bin\MSBuild.exe
```

Run `ms2cc --help` to see every option.

## Projects with setup scripts

Some repositories provide a script that configures package locations, tools, or
environment variables. Run `ms2cc` from the same command prompt after running
that script:

```cmd
cd C:\path\to\repository
call setup-build-environment.cmd
ms2cc Project.sln
```

`ms2cc` does not require changes to your project files or build scripts.

## How it works

`ms2cc` asks MSBuild for the C and C++ files in your project and the compiler
options for each file. It follows referenced projects, checks that every source
file has a compiler command, and then writes the database.

It reads the project settings without compiling object files, so a clean build
is not required. Projects that use C++20 modules or header units may run MSVC's
dependency scanner, but its temporary files are kept outside the project.

Each successful run replaces the output database with the complete set of
projects requested on the command line. If anything fails, `ms2cc` leaves the
existing database unchanged.

## Using the database with clangd

Most editors find `compile_commands.json` automatically when it is placed at the
workspace root. You can also point clangd to its directory:

```text
clangd --compile-commands-dir=C:\path\to\workspace
```

For VS Code with the clangd extension:

```json
{
  "clangd.arguments": [
    "--compile-commands-dir=${workspaceFolder}",
    "--background-index"
  ]
}
```

`ms2cc` keeps the compiler options reported by MSBuild, including precompiled
header settings. Current clangd versions understand most MSVC options and remove
output-only options themselves. Do not remove options from the database unless
clangd reports a specific problem.

See [docs/CLANGD_USAGE.md](docs/CLANGD_USAGE.md) for help with MSVC precompiled
headers and clangd.

## FAQ

### What files can I pass to ms2cc?

`ms2cc` accepts:

- `.sln` and `.slnx` solution files
- `.vcxproj` Visual C++ projects
- `.proj` files that refer to other projects

Referenced projects are followed automatically, so one top-level file is
normally enough.

### Do I need to run a clean build first?

No. `ms2cc` reads the project settings directly from MSBuild.

Packages, imported project files, and generated source files still need to
exist. Run any restore, setup, or source-generation steps required by your
repository before running `ms2cc`.

### Does ms2cc modify or build my project?

No. It reads project information through MSBuild and writes only the requested
compilation database. For C++20 modules and header units, it may run MSVC's
dependency scanner in a temporary directory, but it does not compile object
files or link the project.

### Do I need to specify a configuration and platform?

Not when MSBuild or your project already provides a clear default. Use
`--configuration` and `--platform` when you want a specific configuration or
when a solution has more than one possible choice.

### Why should I run the repository's setup script first?

Some projects use environment variables to locate compilers, packages, SDKs, or
generated files. Running the setup script gives `ms2cc` the same environment as
the normal build.

### What happens to an existing compile_commands.json?

A successful run replaces it with a complete new database for every root passed
on the command line. It is not merged with the old file.

If generation fails, the existing file is left unchanged.

### Does ms2cc support CMake, Make, or other build systems?

No. `ms2cc` is specifically for Visual C++ projects evaluated by MSBuild. CMake
can generate `compile_commands.json` directly, while tools such as Bear may be
more appropriate for other build systems.

### Why is a property from an MSBuild response file missing?

`ms2cc` does not automatically load `MSBuild.rsp` or `Directory.Build.rsp`. Pass
any required property explicitly:

```powershell
ms2cc .\Build.proj --property Name=Value
```

### What if MSBuild cannot be found?

Use a Visual Studio Developer Command Prompt, or pass the path explicitly with
`--msbuild`.

### What if a project takes longer than ten minutes?

Increase the timeout:

```powershell
ms2cc .\Build.proj --timeout 1200
```

The timeout is measured in seconds.

## Development

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## License

MIT. See [LICENSE](LICENSE).
