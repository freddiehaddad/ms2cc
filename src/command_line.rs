mod windows;

use anyhow::{Context, Result, bail};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use crate::paths;

use windows::expand_response_files;
pub use windows::tokenize;

#[derive(Debug, Clone, Default)]
pub struct CompilerEnvironment {
    pub leading_arguments: Vec<String>,
    pub trailing_arguments: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct IncludeDirectories<'a> {
    pub regular: &'a [PathBuf],
    pub external: &'a [PathBuf],
    pub framework: &'a [PathBuf],
}

impl CompilerEnvironment {
    pub fn from_values(leading: Option<&str>, trailing: Option<&str>) -> Result<Self> {
        Ok(Self {
            leading_arguments: leading
                .filter(|value| !value.is_empty())
                .map(tokenize)
                .transpose()?
                .unwrap_or_default(),
            trailing_arguments: trailing
                .filter(|value| !value.is_empty())
                .map(tokenize)
                .transpose()?
                .unwrap_or_default(),
        })
    }
}

pub fn effective_arguments(
    executable: String,
    command_line: &str,
    source_files: &[PathBuf],
    working_directory: &Path,
    include_directories: IncludeDirectories<'_>,
    environment: &CompilerEnvironment,
) -> Result<Vec<Vec<String>>> {
    let mut command_arguments = expand_response_files(&tokenize(command_line)?, working_directory)?;
    if command_arguments
        .first()
        .is_some_and(|argument| is_compiler_executable(argument))
    {
        command_arguments.remove(0);
    }

    let source_lookup: HashMap<String, PathBuf> = source_files
        .iter()
        .map(|source| {
            (
                paths::key(&paths::absolute_lexical(source, working_directory)),
                source.clone(),
            )
        })
        .collect();
    let (base, source_modes) =
        remove_source_operands(&command_arguments, working_directory, &source_lookup);

    let mut prefix = Vec::new();
    prefix.push(executable);
    prefix.extend(environment.leading_arguments.clone());
    prefix.extend(base);

    let mut result = Vec::with_capacity(source_files.len());
    for source in source_files {
        let absolute = paths::absolute_lexical(source, working_directory);
        let source_text = paths::display(&absolute);
        let mut arguments = prefix.clone();
        if let Some(mode) = source_modes.get(&paths::key(&absolute)) {
            arguments.push(format!("{mode}{source_text}"));
        } else {
            arguments.push(source_text);
        }
        arguments.extend(environment.trailing_arguments.clone());
        append_missing_include_paths(&mut arguments, include_directories, working_directory);
        result.push(arguments);
    }
    Ok(result)
}

pub fn add_missing_forced_using(
    arguments: &mut Vec<String>,
    source_file: &Path,
    forced_using_files: &[PathBuf],
    working_directory: &Path,
) {
    let mut known = HashSet::new();
    let mut index = 1;
    while index < arguments.len() {
        if arguments[index].eq_ignore_ascii_case("/FU") {
            if let Some(path) = arguments.get(index + 1) {
                known.insert(paths::key(&paths::absolute_lexical(
                    Path::new(path),
                    working_directory,
                )));
                index += 2;
                continue;
            }
        } else if arguments[index].starts_with("/FU") && arguments[index].len() > 3 {
            known.insert(paths::key(&paths::absolute_lexical(
                Path::new(&arguments[index][3..]),
                working_directory,
            )));
        }
        index += 1;
    }

    let source_key = paths::key(source_file);
    let source_index = arguments
        .iter()
        .position(|argument| {
            paths::key(Path::new(argument)) == source_key
                || argument.get(..3).is_some_and(|prefix| {
                    ["/Tc", "-Tc", "/Tp", "-Tp"]
                        .iter()
                        .any(|expected| prefix.eq_ignore_ascii_case(expected))
                }) && argument
                    .get(3..)
                    .is_some_and(|value| paths::key(Path::new(value)) == source_key)
        })
        .unwrap_or(arguments.len());
    let additions: Vec<_> = forced_using_files
        .iter()
        .map(|path| paths::absolute_lexical(path, working_directory))
        .filter(|path| known.insert(paths::key(path)))
        .map(|path| format!("/FU{}", paths::display(&path)))
        .collect();
    arguments.splice(source_index..source_index, additions);
}

pub fn materialize_module_outputs(
    arguments: &mut Vec<String>,
    source_file: &Path,
    module_output_file: Option<&Path>,
    object_file_name: Option<&Path>,
    source_dependencies_file: Option<&Path>,
    working_directory: &Path,
) -> Result<()> {
    let scan_index = arguments
        .iter()
        .position(|argument| argument.eq_ignore_ascii_case("/scanModules"))
        .with_context(|| {
            format!(
                "resolved module command for {} has no /scanModules placeholder",
                source_file.display()
            )
        })?;
    arguments.remove(scan_index);

    let module_output_file = module_output_file.with_context(|| {
        format!(
            "resolved module command for {} has no module output file",
            source_file.display()
        )
    })?;
    let object_file_name = object_file_name.with_context(|| {
        format!(
            "resolved module command for {} has no object output file",
            source_file.display()
        )
    })?;
    replace_path_option(
        arguments,
        "/ifcOutput",
        module_output_file,
        working_directory,
        source_file,
    )?;
    let object_end = replace_path_option(
        arguments,
        "/Fo",
        object_file_name,
        working_directory,
        source_file,
    )?;

    if let Some(path) = source_dependencies_file {
        if has_option(arguments, "/sourceDependencies") {
            replace_path_option(
                arguments,
                "/sourceDependencies",
                path,
                working_directory,
                source_file,
            )?;
        } else {
            let path = paths::absolute_lexical(path, working_directory);
            arguments.splice(
                object_end..object_end,
                ["/sourceDependencies".to_owned(), paths::display(&path)],
            );
        }
    }
    Ok(())
}

fn has_option(arguments: &[String], option: &str) -> bool {
    arguments.iter().any(|argument| {
        argument.eq_ignore_ascii_case(option)
            || argument
                .get(..option.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(option))
                && argument.len() > option.len()
    })
}

fn replace_path_option(
    arguments: &mut [String],
    option: &str,
    path: &Path,
    working_directory: &Path,
    source_file: &Path,
) -> Result<usize> {
    let path = paths::display(&paths::absolute_lexical(path, working_directory));
    for index in 1..arguments.len() {
        if arguments[index].eq_ignore_ascii_case(option) {
            let operand = arguments.get_mut(index + 1).with_context(|| {
                format!(
                    "{option} has no operand in module command for {}",
                    source_file.display()
                )
            })?;
            *operand = path;
            return Ok(index + 2);
        }
        if arguments[index]
            .get(..option.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(option))
            && arguments[index].len() > option.len()
        {
            arguments[index] = format!("{option}{path}");
            return Ok(index + 1);
        }
    }
    bail!(
        "resolved module command for {} has no {option} option",
        source_file.display()
    )
}

fn is_compiler_executable(argument: &str) -> bool {
    Path::new(argument)
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            value.eq_ignore_ascii_case("cl")
                || value.eq_ignore_ascii_case("cl.exe")
                || value.eq_ignore_ascii_case("clang-cl")
                || value.eq_ignore_ascii_case("clang-cl.exe")
        })
}

fn remove_source_operands(
    arguments: &[String],
    working_directory: &Path,
    source_lookup: &HashMap<String, PathBuf>,
) -> (Vec<String>, HashMap<String, String>) {
    let mut base = Vec::new();
    let mut source_modes = HashMap::new();
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if matches!(argument.as_str(), "/Tc" | "-Tc" | "/Tp" | "-Tp") && index + 1 < arguments.len()
        {
            let candidate =
                paths::absolute_lexical(Path::new(&arguments[index + 1]), working_directory);
            let key = paths::key(&candidate);
            if source_lookup.contains_key(&key) {
                source_modes.insert(key, argument.clone());
                index += 2;
                continue;
            }
        }

        if takes_next_operand(argument) && index + 1 < arguments.len() {
            base.push(argument.clone());
            base.push(arguments[index + 1].clone());
            index += 2;
            continue;
        }

        if let Some(prefix) = argument.get(..3)
            && (matches_ignore_ascii_case(prefix, "/Tc")
                || matches_ignore_ascii_case(prefix, "-Tc")
                || matches_ignore_ascii_case(prefix, "/Tp")
                || matches_ignore_ascii_case(prefix, "-Tp"))
        {
            let candidate = paths::absolute_lexical(Path::new(&argument[3..]), working_directory);
            let key = paths::key(&candidate);
            if source_lookup.contains_key(&key) {
                source_modes.insert(key, prefix.to_owned());
                index += 1;
                continue;
            }
        }

        let candidate = paths::absolute_lexical(Path::new(argument), working_directory);
        if source_lookup.contains_key(&paths::key(&candidate)) {
            index += 1;
            continue;
        }
        base.push(argument.clone());
        index += 1;
    }
    (base, source_modes)
}

fn takes_next_operand(argument: &str) -> bool {
    [
        "/D",
        "-D",
        "/I",
        "-I",
        "/U",
        "-U",
        "/FI",
        "/FU",
        "/AI",
        "/external:I",
        "/sourceDependencies",
        "/scanDependencies",
        "/ifcOutput",
        "/reference",
        "/headerUnit",
    ]
    .iter()
    .any(|option| argument.eq_ignore_ascii_case(option))
}

fn matches_ignore_ascii_case(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn append_missing_include_paths(
    arguments: &mut Vec<String>,
    include_directories: IncludeDirectories<'_>,
    working_directory: &Path,
) {
    let mut known = HashSet::new();
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if (argument.eq_ignore_ascii_case("/I")
            || argument.eq_ignore_ascii_case("-I")
            || argument.eq_ignore_ascii_case("/external:I"))
            && let Some(path) = arguments.get(index + 1)
        {
            known.insert(paths::key(&paths::absolute_lexical(
                Path::new(path),
                working_directory,
            )));
            index += 2;
            continue;
        }
        for prefix in ["/external:I", "/I", "-I"] {
            if argument.len() > prefix.len()
                && argument
                    .get(..prefix.len())
                    .is_some_and(|value| value.eq_ignore_ascii_case(prefix))
            {
                known.insert(paths::key(&paths::absolute_lexical(
                    Path::new(&argument[prefix.len()..]),
                    working_directory,
                )));
                break;
            }
        }
        index += 1;
    }

    let external: HashSet<_> = include_directories
        .external
        .iter()
        .map(|path| paths::key(&paths::absolute_lexical(path, working_directory)))
        .collect();
    let ignore_standard = arguments
        .iter()
        .any(|argument| argument.eq_ignore_ascii_case("/X") || argument.eq_ignore_ascii_case("-X"));

    if !ignore_standard {
        for include in include_directories.regular {
            let absolute = paths::absolute_lexical(include, working_directory);
            if known.insert(paths::key(&absolute)) {
                let option = if external.contains(&paths::key(&absolute)) {
                    "/external:I"
                } else {
                    "/I"
                };
                arguments.push(format!("{option}{}", paths::display(&absolute)));
            }
        }
        for include in include_directories.framework {
            let absolute = paths::absolute_lexical(include, working_directory);
            if known.insert(paths::key(&absolute)) {
                arguments.push(format!("/I{}", paths::display(&absolute)));
            }
        }
    }
    for include in include_directories.external {
        let absolute = paths::absolute_lexical(include, working_directory);
        if known.insert(paths::key(&absolute)) {
            arguments.push(format!("/external:I{}", paths::display(&absolute)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_sources_using_authoritative_file_list() {
        let working = Path::new(r"C:\project");
        let sources = vec![PathBuf::from("a.cpp"), PathBuf::from("b.cpp")];
        let result = effective_arguments(
            r"C:\tools\cl.exe".to_owned(),
            r#"/c /D VALUE /TP"#,
            &sources,
            working,
            IncludeDirectories::default(),
            &CompilerEnvironment::default(),
        )
        .unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].last().unwrap(), r"C:\project\a.cpp");
        assert_eq!(result[1].last().unwrap(), r"C:\project\b.cpp");
    }

    #[test]
    fn source_looking_option_operands_are_not_removed() {
        let working = Path::new(r"C:\project");
        let sources = vec![PathBuf::from("actual.cpp")];
        let result = effective_arguments(
            r"C:\tools\cl.exe".to_owned(),
            r#"/c /FI generated.cpp /D value.cpp actual.cpp"#,
            &sources,
            working,
            IncludeDirectories::default(),
            &CompilerEnvironment::default(),
        )
        .unwrap();
        assert!(
            result[0]
                .windows(2)
                .any(|pair| pair == ["/FI", "generated.cpp"])
        );
        assert!(result[0].windows(2).any(|pair| pair == ["/D", "value.cpp"]));
    }

    #[test]
    fn ignore_standard_includes_preserves_only_external_directories() {
        let working = Path::new(r"C:\project");
        let sources = vec![PathBuf::from("main.cpp")];
        let result = effective_arguments(
            r"C:\tools\cl.exe".to_owned(),
            r#"/c -X /D名=1 main.cpp"#,
            &sources,
            working,
            IncludeDirectories {
                regular: &[PathBuf::from(r"C:\regular")],
                external: &[PathBuf::from(r"C:\external")],
                framework: &[PathBuf::from(r"C:\framework")],
            },
            &CompilerEnvironment::default(),
        )
        .unwrap();

        assert!(!result[0].iter().any(|value| value.contains(r"C:\regular")));
        assert!(
            !result[0]
                .iter()
                .any(|value| value.contains(r"C:\framework"))
        );
        assert!(
            result[0]
                .iter()
                .any(|value| value == r"/external:IC:\external")
        );
    }

    #[test]
    fn overlapping_external_directories_keep_external_classification() {
        let working = Path::new(r"C:\project");
        let sources = vec![PathBuf::from("main.cpp")];
        let external = PathBuf::from(r"C:\toolchain\include");
        let regular = PathBuf::from(r"C:\project\include");
        let result = effective_arguments(
            r"C:\tools\cl.exe".to_owned(),
            "main.cpp",
            &sources,
            working,
            IncludeDirectories {
                regular: &[external.clone(), regular.clone()],
                external: &[external],
                framework: &[],
            },
            &CompilerEnvironment::default(),
        )
        .unwrap();

        assert!(
            result[0]
                .iter()
                .any(|value| value == r"/external:IC:\toolchain\include")
        );
        assert!(
            result[0]
                .iter()
                .any(|value| value == r"/IC:\project\include")
        );
    }

    #[test]
    fn materializes_missing_forced_using_files_before_the_source() {
        let working = Path::new(r"C:\project");
        let source = Path::new(r"C:\project\main.cpp");
        let mut arguments = vec![
            r"C:\tools\cl.exe".to_owned(),
            r"/FUC:\refs\existing.dll".to_owned(),
            source.display().to_string(),
        ];
        add_missing_forced_using(
            &mut arguments,
            source,
            &[
                PathBuf::from(r"C:\refs\existing.dll"),
                PathBuf::from(r"C:\refs\new.dll"),
            ],
            working,
        );
        assert_eq!(
            arguments,
            vec![
                r"C:\tools\cl.exe",
                r"/FUC:\refs\existing.dll",
                r"/FUC:\refs\new.dll",
                r"C:\project\main.cpp",
            ]
        );
    }

    #[test]
    fn replaces_design_time_module_placeholders_with_compile_outputs() {
        let source = Path::new(r"C:\project\Sleeper.h");
        let mut arguments = vec![
            "cl.exe".to_owned(),
            "/scanModules".to_owned(),
            "/c".to_owned(),
            "/ifcOutput".to_owned(),
            "obj\\".to_owned(),
            "/Foobj\\".to_owned(),
            "/Gd".to_owned(),
            source.display().to_string(),
        ];

        materialize_module_outputs(
            &mut arguments,
            source,
            Some(Path::new(r"obj\Sleeper.h.ifc")),
            Some(Path::new(r"obj\Sleeper.h.obj")),
            Some(Path::new(r"obj\Sleeper.h.ifc.d.json")),
            Path::new(r"C:\project"),
        )
        .unwrap();

        assert_eq!(
            arguments,
            vec![
                "cl.exe",
                "/c",
                "/ifcOutput",
                r"C:\project\obj\Sleeper.h.ifc",
                r"/FoC:\project\obj\Sleeper.h.obj",
                "/sourceDependencies",
                r"C:\project\obj\Sleeper.h.ifc.d.json",
                "/Gd",
                r"C:\project\Sleeper.h",
            ]
        );
    }
}
