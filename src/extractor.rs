use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::{
    command_line,
    discovery::{property_argument, property_arguments},
    model::{CommandRecord, CompileCommand, ProjectInstance, SourceKey},
    msbuild::MsbuildRunner,
    paths, query,
};

const TARGET_NAME: &str = "Ms2ccGetCompileCommands";
static SCRATCH_ID: AtomicUsize = AtomicUsize::new(1);

#[derive(Debug, Default)]
struct SourceMetadata {
    source: PathBuf,
    forced_using: Vec<PathBuf>,
    compile_as: String,
    compiler_iteration: String,
    module_output_file: Option<PathBuf>,
    object_file_name: Option<PathBuf>,
    generate_source_dependencies: bool,
    source_dependencies_file: Option<PathBuf>,
}

#[derive(Debug, Clone)]
enum PublicBmi {
    HeaderUnit { source: PathBuf, bmi: PathBuf },
    Module { name: String, bmi: PathBuf },
}

#[derive(Debug, Default)]
struct ReferencedBmis {
    header_units: Vec<(PathBuf, PathBuf)>,
    modules: Vec<(String, PathBuf)>,
}

#[derive(Debug)]
pub struct Extraction {
    pub commands: Vec<CommandRecord>,
    expected_sources: Vec<SourceKey>,
    public_bmis: Vec<PublicBmi>,
}

impl Extraction {
    pub fn prove_complete(&self) -> Result<()> {
        let expected = counts(self.expected_sources.iter().cloned());
        let actual = counts(self.commands.iter().map(CommandRecord::source_key));
        let ambiguous: Vec<_> = expected
            .iter()
            .chain(actual.iter())
            .filter(|(_, count)| **count > 1)
            .collect();
        if !ambiguous.is_empty() {
            let mut message = format!(
                "structured extraction is ambiguous for {} project/source key(s)",
                ambiguous.len()
            );
            for (key, count) in ambiguous.into_iter().take(20) {
                message.push_str(&format!(
                    "\n  x{count}: {} [{}] {}",
                    key.project, key.properties, key.source
                ));
            }
            bail!(message);
        }
        let missing = subtract(&expected, &actual);
        let extra = subtract(&actual, &expected);
        if missing.is_empty() && extra.is_empty() {
            return Ok(());
        }

        let mut message = format!(
            "structured extraction is incomplete: {} missing and {} extra project/source occurrence(s)",
            missing.values().sum::<usize>(),
            extra.values().sum::<usize>()
        );
        for (label, entries) in [("missing", missing), ("extra", extra)] {
            for (key, count) in entries.into_iter().take(20) {
                message.push_str(&format!(
                    "\n  {label} x{count}: {} [{}] {}",
                    key.project, key.properties, key.source
                ));
            }
        }
        bail!(message)
    }

    pub fn expected_source_count(&self) -> usize {
        self.expected_sources.len()
    }
}

pub fn extract_all(
    runner: &MsbuildRunner,
    adapter_path: &Path,
    projects: &[ProjectInstance],
    references: &HashMap<String, Vec<String>>,
) -> Result<Extraction> {
    let completed = AtomicUsize::new(0);
    let total = projects.len();
    let results: Vec<Result<Extraction>> = projects
        .par_iter()
        .map(|project| {
            let result = extract_project(runner, adapter_path, project, None);
            let current = completed.fetch_add(1, Ordering::Relaxed) + 1;
            if current == total || current.is_multiple_of(50) {
                eprintln!("Extracted {current}/{total} project instance(s)");
            }
            result
        })
        .collect();

    let mut project_extractions: Vec<Extraction> =
        results.into_iter().collect::<Result<Vec<_>>>()?;
    let public_by_project: HashMap<_, _> = projects
        .iter()
        .zip(&project_extractions)
        .map(|(project, extraction)| (project.key(), extraction.public_bmis.clone()))
        .collect();
    let reruns: Vec<_> = projects
        .par_iter()
        .enumerate()
        .filter_map(|(index, project)| {
            let mut referenced = ReferencedBmis::default();
            for child in references.get(&project.key()).into_iter().flatten() {
                for public in public_by_project.get(child).into_iter().flatten() {
                    match public {
                        PublicBmi::HeaderUnit { source, bmi } => {
                            referenced.header_units.push((source.clone(), bmi.clone()));
                        }
                        PublicBmi::Module { name, bmi } => {
                            referenced.modules.push((name.clone(), bmi.clone()));
                        }
                    }
                }
            }
            deduplicate_referenced_bmis(&mut referenced);
            (!referenced.header_units.is_empty() || !referenced.modules.is_empty()).then(|| {
                (
                    index,
                    extract_project(runner, adapter_path, project, Some(&referenced)),
                )
            })
        })
        .collect();
    for (index, result) in reruns {
        project_extractions[index] = result?;
    }

    let mut extraction = Extraction {
        commands: Vec::new(),
        expected_sources: Vec::new(),
        public_bmis: Vec::new(),
    };
    for mut project in project_extractions {
        extraction.commands.append(&mut project.commands);
        extraction
            .expected_sources
            .append(&mut project.expected_sources);
        extraction.public_bmis.append(&mut project.public_bmis);
    }
    Ok(extraction)
}

fn extract_project(
    runner: &MsbuildRunner,
    adapter_path: &Path,
    project: &ProjectInstance,
    referenced_bmis: Option<&ReferencedBmis>,
) -> Result<Extraction> {
    let mut arguments = property_arguments(&project.properties);
    arguments.push(property_argument("DesignTimeBuild", "true"));
    arguments.push(property_argument("BuildingInsideVisualStudio", "true"));
    arguments.push(property_argument("BuildProjectReferences", "false"));
    arguments.push(property_argument("BuildingProject", "true"));
    arguments.push(property_argument(
        "AlwaysResolveReferencesDesignTime",
        "true",
    ));
    arguments.push(property_argument(
        "ShouldUnsetParentConfigurationAndPlatform",
        "false",
    ));
    arguments.push(property_argument("_FindDependencies", "true"));
    arguments.push(property_argument(
        "FindDependenciesOfExternallyResolvedReferences",
        "true",
    ));
    if let Some(referenced_bmis) = referenced_bmis {
        arguments.extend(referenced_bmi_arguments(referenced_bmis));
    }
    let scratch_id = SCRATCH_ID.fetch_add(1, Ordering::Relaxed);
    let scratch_directory = runner.temporary_path(format!("module-scan-{scratch_id}"));
    arguments.push(property_argument(
        "Ms2ccScratchDirectory",
        &scratch_directory.to_string_lossy(),
    ));
    arguments.push(property_argument(
        "ForceImportAfterCppTargets",
        &adapter_path.to_string_lossy(),
    ));
    arguments.push(format!("-target:{TARGET_NAME}"));
    arguments.push(format!("-getTargetResult:{TARGET_NAME}"));

    let document = runner.query_json(
        &project.path,
        &arguments,
        &format!("Extracting {}", project.path.display()),
    )?;
    parse_project_result(&document, project, &scratch_directory)
        .with_context(|| format!("failed to decode commands for {}", project.path.display()))
}

fn parse_project_result(
    document: &Value,
    project: &ProjectInstance,
    scratch_directory: &Path,
) -> Result<Extraction> {
    let items = query::target_items(document, TARGET_NAME)?;
    let mut include_paths = Vec::new();
    let mut external_include_paths = Vec::new();
    let mut framework_include_paths = Vec::new();
    let mut environment = command_line::CompilerEnvironment::default();
    let mut synthetic_source_counts: HashMap<String, usize> = HashMap::new();
    let mut has_project_references = false;
    let mut reference_resolution_disabled = false;
    let mut expected_sources = Vec::new();
    let mut metadata_by_source: HashMap<String, SourceMetadata> = HashMap::new();
    let mut public_header_units = HashSet::new();
    let mut public_modules = HashSet::new();
    let mut command_items = Vec::new();

    for item in items {
        let kind = query::metadata(item, "Kind").unwrap_or_default();
        if matches!(
            kind,
            "Directories" | "Source" | "Command" | "PublicHeaderUnit" | "PublicModule"
        ) {
            let returned_project = query::metadata(item, "ProjectFile")
                .context("adapter item is missing ProjectFile metadata")?;
            if paths::key(Path::new(returned_project)) != paths::key(&project.path) {
                bail!(
                    "adapter returned data for {} while evaluating {}",
                    returned_project,
                    project.path.display()
                );
            }
        }
        match kind {
            "Directories" => {
                include_paths.extend(
                    query::metadata(item, "IncludePath")
                        .map(split_msbuild_list)
                        .unwrap_or_default()
                        .into_iter()
                        .map(PathBuf::from),
                );
                let use_env = query::metadata(item, "UseEnv")
                    .is_some_and(|value| value.eq_ignore_ascii_case("true"));
                external_include_paths.extend(
                    query::metadata(
                        item,
                        if use_env {
                            "EnvironmentExternalInclude"
                        } else {
                            "ExternalIncludePath"
                        },
                    )
                    .map(split_msbuild_list)
                    .unwrap_or_default()
                    .into_iter()
                    .map(PathBuf::from),
                );
                framework_include_paths.extend(
                    query::metadata(item, "FrameworkIncludePath")
                        .map(split_msbuild_list)
                        .unwrap_or_default()
                        .into_iter()
                        .map(PathBuf::from),
                );
                environment = command_line::CompilerEnvironment::from_values(
                    query::metadata(item, "EnvironmentCL"),
                    query::metadata(item, "EnvironmentTrailingCL"),
                )?;
                has_project_references = query::metadata(item, "HasProjectReferences")
                    .is_some_and(|value| value.eq_ignore_ascii_case("true"));
                reference_resolution_disabled =
                    query::metadata(item, "ReferenceResolutionDisabled")
                        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
            }
            "Source" => {
                if query::metadata(item, "ExcludedFromBuild")
                    .is_some_and(|value| value.eq_ignore_ascii_case("true"))
                {
                    continue;
                }
                let source = query::metadata(item, "Source")
                    .context("source adapter item is missing Source metadata")?;
                if query::metadata(item, "ConfigurationOptions")
                    .is_some_and(|value| value.eq_ignore_ascii_case("true"))
                {
                    *synthetic_source_counts
                        .entry(paths::key(Path::new(source)))
                        .or_insert(0) += 1;
                    continue;
                }
                expected_sources.push(SourceKey::new(
                    &project.path,
                    &project.properties,
                    Path::new(source),
                ));
                metadata_by_source.insert(
                    paths::key(Path::new(source)),
                    SourceMetadata {
                        source: PathBuf::from(source),
                        forced_using: query::metadata(item, "ForcedUsingFiles")
                            .map(split_msbuild_list)
                            .unwrap_or_default()
                            .into_iter()
                            .map(PathBuf::from)
                            .collect(),
                        compile_as: query::metadata(item, "CompileAs")
                            .unwrap_or_default()
                            .to_owned(),
                        compiler_iteration: query::metadata(item, "CompilerIteration")
                            .unwrap_or_default()
                            .to_owned(),
                        module_output_file: nonempty_path(item, "ModuleOutputFile"),
                        object_file_name: nonempty_path(item, "ObjectFileName"),
                        generate_source_dependencies: query::metadata(
                            item,
                            "GenerateSourceDependencies",
                        )
                        .is_some_and(|value| value.eq_ignore_ascii_case("true")),
                        source_dependencies_file: nonempty_path(item, "SourceDependenciesFile"),
                    },
                );
            }
            "PublicHeaderUnit" => {
                if let Some(bmi) = query::metadata(item, "Bmi") {
                    public_header_units.insert(paths::key(Path::new(bmi)));
                }
            }
            "PublicModule" => {
                if let Some(bmi) = query::metadata(item, "Bmi") {
                    public_modules.insert(paths::key(Path::new(bmi)));
                }
            }
            "Command" => command_items.push(item),
            _ => {}
        }
    }

    deduplicate_paths(&mut include_paths);
    deduplicate_paths(&mut external_include_paths);
    deduplicate_paths(&mut framework_include_paths);
    let module_names = read_module_names(scratch_directory)?;
    let mut public_bmis = Vec::new();
    for (source, metadata) in &metadata_by_source {
        let Some(output) = &metadata.module_output_file else {
            continue;
        };
        let output_key = paths::key(output);
        if public_header_units.contains(&output_key)
            && metadata
                .compile_as
                .eq_ignore_ascii_case("CompileAsHeaderUnit")
        {
            public_bmis.push(PublicBmi::HeaderUnit {
                source: metadata.source.clone(),
                bmi: output.clone(),
            });
        } else if public_modules.contains(&output_key)
            && metadata
                .compile_as
                .eq_ignore_ascii_case("CompileAsCppModule")
        {
            let name = module_names.get(source).with_context(|| {
                format!("could not determine the public module name for {source}")
            })?;
            public_bmis.push(PublicBmi::Module {
                name: name.clone(),
                bmi: output.clone(),
            });
        }
    }
    let mut commands = Vec::new();
    for item in command_items {
        if has_project_references
            && (reference_resolution_disabled
                || !query::metadata(item, "ContainsReferencedProjectsData")
                    .is_some_and(|value| value.eq_ignore_ascii_case("true")))
        {
            bail!(
                "MSBuild did not provide referenced-project compiler data for {}",
                project.path.display()
            );
        }
        let working_directory = query::metadata(item, "WorkingDirectory")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                project
                    .path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .to_path_buf()
            });
        let working_directory = paths::absolute_lexical(
            &working_directory,
            project.path.parent().unwrap_or(Path::new(".")),
        );
        let mut files: Vec<PathBuf> = query::metadata(item, "Files")
            .unwrap_or_default()
            .split(';')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(unescape_msbuild)
            .map(PathBuf::from)
            .collect();
        if query::metadata(item, "ConfigurationOptions")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
        {
            files.retain(|source| {
                let key = paths::key(&paths::absolute_lexical(source, &working_directory));
                !synthetic_source_counts.contains_key(&key) || metadata_by_source.contains_key(&key)
            });
        }
        if files.is_empty() || files.iter().all(|path| is_shader(path)) {
            continue;
        }

        let executable = resolve_compiler(item, &working_directory)?;
        let command_line = query::metadata(item, "CommandLine")
            .context("command adapter item is missing CommandLine metadata")?;
        let argument_sets = command_line::effective_arguments(
            paths::display(&executable),
            command_line,
            &files,
            &working_directory,
            command_line::IncludeDirectories {
                regular: &include_paths,
                external: &external_include_paths,
                framework: &framework_include_paths,
            },
            &environment,
        )?;

        for (source, mut arguments) in files.iter().zip(argument_sets) {
            let absolute_source = paths::absolute_lexical(source, &working_directory);
            if let Some(metadata) = metadata_by_source.get(&paths::key(&absolute_source)) {
                command_line::add_missing_forced_using(
                    &mut arguments,
                    &absolute_source,
                    &metadata.forced_using,
                    &working_directory,
                );
                if metadata.compiler_iteration.eq_ignore_ascii_case("Modules") {
                    let source_dependencies_file =
                        if metadata.generate_source_dependencies {
                            Some(metadata.source_dependencies_file.as_deref().context(
                                "module source is missing SourceDependenciesFile metadata",
                            )?)
                        } else {
                            None
                        };
                    command_line::materialize_module_outputs(
                        &mut arguments,
                        &absolute_source,
                        metadata.module_output_file.as_deref(),
                        metadata.object_file_name.as_deref(),
                        source_dependencies_file,
                        &working_directory,
                    )?;
                }
            }
            if arguments
                .iter()
                .any(|argument| argument.eq_ignore_ascii_case("/scanModules"))
            {
                bail!(
                    "module dependency scan did not resolve a compile command for {}",
                    absolute_source.display()
                );
            }
            commands.push(CommandRecord {
                command: CompileCommand {
                    directory: paths::display(&working_directory),
                    arguments,
                    file: paths::display(&absolute_source),
                },
                project: project.path.clone(),
                properties: project.properties.clone(),
            });
        }
    }

    Ok(Extraction {
        commands,
        expected_sources,
        public_bmis,
    })
}

fn referenced_bmi_arguments(referenced: &ReferencedBmis) -> Vec<String> {
    let header_units = referenced
        .header_units
        .iter()
        .map(|(source, bmi)| format!("{}={}", paths::display(source), paths::display(bmi)))
        .collect::<Vec<_>>();
    let header_bmis = referenced
        .header_units
        .iter()
        .map(|(_, bmi)| paths::display(bmi))
        .collect::<Vec<_>>();
    let modules = referenced
        .modules
        .iter()
        .map(|(name, bmi)| format!("{name}={}", paths::display(bmi)))
        .collect::<Vec<_>>();
    let module_bmis = referenced
        .modules
        .iter()
        .map(|(_, bmi)| paths::display(bmi))
        .collect::<Vec<_>>();

    [
        ("Ms2ccReferencedHeaderUnits", header_units.join(";")),
        ("Ms2ccReferencedHeaderUnitBMIs", header_bmis.join(";")),
        ("Ms2ccReferencedModules", modules.join(";")),
        ("Ms2ccReferencedModuleBMIs", module_bmis.join(";")),
    ]
    .into_iter()
    .filter(|(_, value)| !value.is_empty())
    .map(|(name, value)| property_argument(name, &value))
    .collect()
}

fn deduplicate_referenced_bmis(referenced: &mut ReferencedBmis) {
    let mut headers = HashSet::new();
    referenced
        .header_units
        .retain(|(source, bmi)| headers.insert((paths::key(source), paths::key(bmi))));
    let mut modules = HashSet::new();
    referenced
        .modules
        .retain(|(name, bmi)| modules.insert((name.to_ascii_lowercase(), paths::key(bmi))));
}

fn read_module_names(directory: &Path) -> Result<HashMap<String, String>> {
    let mut result = HashMap::new();
    if !directory.is_dir() {
        return Ok(result);
    }
    for entry in fs::read_dir(directory).with_context(|| {
        format!(
            "failed to read module scan directory {}",
            directory.display()
        )
    })? {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.to_ascii_lowercase().ends_with(".module.json"))
        {
            continue;
        }
        let document: Value = serde_json::from_slice(
            &fs::read(&path)
                .with_context(|| format!("failed to read module scan {}", path.display()))?,
        )
        .with_context(|| format!("failed to parse module scan {}", path.display()))?;
        if let Some(data) = document.get("Data").or_else(|| document.get("data")) {
            let source = data
                .get("Source")
                .or_else(|| data.get("source"))
                .and_then(Value::as_str);
            let module = data
                .get("ProvidedModule")
                .or_else(|| data.get("provided-module"))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty());
            if let (Some(source), Some(module)) = (source, module) {
                result.insert(paths::key(Path::new(source)), module.to_owned());
            }
        }
        for rule in document
            .get("rules")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            for provided in rule
                .get("provides")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let source = provided.get("source-path").and_then(Value::as_str);
                let module = provided
                    .get("logical-name")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                if let (Some(source), Some(module)) = (source, module) {
                    result.insert(paths::key(Path::new(source)), module.to_owned());
                }
            }
        }
    }
    Ok(result)
}

fn nonempty_path(item: &Value, name: &str) -> Option<PathBuf> {
    query::metadata(item, name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn resolve_compiler(item: &Value, working_directory: &Path) -> Result<PathBuf> {
    let tool_exe = query::metadata(item, "CLToolExe")
        .filter(|value| !value.is_empty())
        .unwrap_or("cl.exe");
    let mut candidates = Vec::new();
    if let Some(tool_path) = query::metadata(item, "ToolPath").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(tool_path);
        if path.is_dir() {
            candidates.push(path.join(tool_exe));
        } else {
            candidates.push(path.clone());
            candidates.push(path.join(tool_exe));
        }
    }
    if let Some(tool_path) = query::metadata(item, "CLToolPath").filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(tool_path).join(tool_exe));
    }
    if let Some(executable_path) =
        query::metadata(item, "ExecutablePath").filter(|value| !value.is_empty())
    {
        candidates.extend(
            split_msbuild_list(executable_path)
                .into_iter()
                .map(PathBuf::from)
                .map(|path| path.join(tool_exe)),
        );
    }
    if let Some(path) = find_on_path(tool_exe) {
        candidates.push(path);
    }

    for candidate in &candidates {
        let absolute = paths::absolute_lexical(candidate, working_directory);
        if absolute.is_file() {
            return Ok(absolute);
        }
    }

    bail!(
        "could not resolve compiler '{}'; candidates: {}",
        tool_exe,
        candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?).find_map(|directory| {
        let candidate = directory.join(name);
        candidate.is_file().then_some(candidate)
    })
}

fn split_msbuild_list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(unescape_msbuild)
        .collect()
}

fn unescape_msbuild(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut result = Vec::with_capacity(value.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(hex) = u8::from_str_radix(&value[index + 1..index + 3], 16)
        {
            result.push(hex);
            index += 3;
        } else {
            result.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(result).unwrap_or_else(|_| value.to_owned())
}

fn is_shader(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "hlsl" | "fx" | "hlsli"))
}

fn deduplicate_paths(values: &mut Vec<PathBuf>) {
    let mut seen = HashSet::new();
    values.retain(|path| seen.insert(paths::key(path)));
}

fn counts(values: impl IntoIterator<Item = SourceKey>) -> BTreeMap<SourceKey, usize> {
    let mut result = BTreeMap::new();
    for value in values {
        *result.entry(value).or_insert(0) += 1;
    }
    result
}

fn subtract(
    left: &BTreeMap<SourceKey, usize>,
    right: &BTreeMap<SourceKey, usize>,
) -> BTreeMap<SourceKey, usize> {
    left.iter()
        .filter_map(|(key, left_count)| {
            let difference = left_count.saturating_sub(*right.get(key).unwrap_or(&0));
            (difference > 0).then_some((key.clone(), difference))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescapes_msbuild_hex_sequences() {
        assert_eq!(unescape_msbuild(r"C:\a%20b\c"), r"C:\a b\c");
    }

    #[test]
    fn multiset_subtraction_preserves_duplicate_counts() {
        let key = SourceKey {
            project: "p".to_owned(),
            properties: String::new(),
            source: "s".to_owned(),
        };
        let left = counts([key.clone(), key.clone()]);
        let right = counts([key.clone()]);
        assert_eq!(subtract(&left, &right).get(&key), Some(&1));
    }
}
