# Integration tests

The Windows integration tests invoke an installed MSBuild 17.8 or newer against
the synthetic Visual C++ project under `fixtures\native`. They verify structured
`GetClCommandLines` extraction, project-reference discovery, command fidelity,
complete replacement, failure safety, and the intentionally small CLI.

Run all tests:

```powershell
cargo test
```

The fixtures are ordinary MSBuild projects. Most tests run only Visual C++
design-time targets. The module fixture also invokes MSVC's dependency-only
scanner and verifies that its outputs stay in ms2cc's temporary directory.
