# Using clangd with ms2cc

## Generate the database

Run `ms2cc` from the same initialized environment used by MSBuild:

```powershell
ms2cc .\YourSolution.sln -c Release --platform x64
```

For a traversal project:

```powershell
ms2cc .\Build.proj -c Release --platform x64
```

The output defaults to `compile_commands.json` in the current directory. A
successful invocation regenerates the complete requested scope and atomically
replaces the prior file.

## Start clangd

Place the database at the workspace root or point clangd at its directory:

```text
clangd --compile-commands-dir=C:\path\to\workspace
```

For VS Code:

```json
{
  "C_Cpp.intelliSenseEngine": "disabled",
  "clangd.arguments": [
    "--compile-commands-dir=${workspaceFolder}",
    "--background-index"
  ]
}
```

## MSVC arguments

`ms2cc` preserves the command returned by Visual C++ design-time evaluation,
including `/Fo`, `/Fd`, `/Yc`, `/Yu`, `/Fp`, and `/FI`.

Clangd performs its own frontend-only command adjustment and removes output
arguments. Recent clangd versions also understand clang-cl PCH switches. Do not
remove PCH switches unless diagnostics prove that a specific project's
MSVC-generated PCH is incompatible.

If that occurs, use a narrowly scoped `.clangd` rule:

```yaml
If:
  PathMatch: path/to/affected/files/.*
CompileFlags:
  Remove:
    - /Yc*
    - /Yu*
    - /Fp*
```

`/Yu<header>` can supply implicit-header semantics. Before removing it, verify
that the source includes the header directly or that the command also contains
an equivalent `/FI<header>` argument.

## Troubleshooting

### MSBuild cannot be found

Run from a Visual Studio Developer Command Prompt or pass the desired executable:

```powershell
ms2cc .\App.vcxproj --msbuild C:\BuildTools\MSBuild\Current\Bin\MSBuild.exe
```

Repository-provided `.cmd` wrappers are supported.

### Configuration is ambiguous

Select it explicitly:

```powershell
ms2cc .\App.sln -c Debug --platform x64
```

### Project imports or generated files are missing

Restore packages and run the repository's normal generation/bootstrap step
first. `ms2cc` does not compile object code or recreate build-generated
prerequisites. It may invoke MSVC's dependency-only scanner for C++20 modules
and header units; scanner failures are reported rather than producing commands
with unresolved module mappings.

### Extraction times out

Increase the per-process bound:

```powershell
ms2cc .\LargeTree.proj --timeout 1200
```

The tool emits periodic progress heartbeats and terminates the complete MSBuild
process tree when the bound is exceeded.

### clangd reports missing standard headers

Confirm that `arguments` contains the effective Visual C++ and Windows SDK
include directories and that the compiler path exists. Run `ms2cc` from the
initialized build environment so repository-specific `INCLUDE` settings are
available.

### Existing entries disappear

This is intentional. Design-time extraction produces the complete selected
scope, so each run replaces the database rather than merging stale commands.
Pass every desired project or solution root in the same invocation.
