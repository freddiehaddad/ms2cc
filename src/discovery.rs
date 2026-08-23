use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
};

use crate::{
    model::{ProjectInstance, Properties},
    msbuild::MsbuildRunner,
    paths, query,
    solution::{self, set_property},
};

#[derive(Debug)]
pub struct Discovery {
    pub projects: Vec<ProjectInstance>,
    pub visited_projects: Vec<PathBuf>,
    pub references: HashMap<String, Vec<String>>,
}

pub fn discover(
    runner: &MsbuildRunner,
    inputs: &[PathBuf],
    base_properties: &Properties,
) -> Result<Discovery> {
    let mut queue = VecDeque::new();
    let mut scheduled = HashSet::new();
    let mut visited: HashMap<String, PathBuf> = HashMap::new();
    for input in inputs {
        let roots = solution::projects(input, base_properties)?;
        for path in roots.protected_paths {
            visited.insert(paths::key(&path), path);
        }
        for project in roots.projects {
            if scheduled.insert(project.key()) {
                queue.push_back(project);
            }
        }
    }

    let mut native: HashMap<String, ProjectInstance> = HashMap::new();
    let mut references: HashMap<String, Vec<String>> = HashMap::new();
    let mut processed = 0usize;

    while !queue.is_empty() {
        let mut batch = Vec::with_capacity(64);
        while batch.len() < 64
            && let Some(requested) = queue.pop_front()
        {
            batch.push(requested);
        }
        let evaluations: Vec<_> = batch
            .par_iter()
            .map(|project| evaluate_project(runner, project))
            .collect();

        for (requested, evaluation) in batch.into_iter().zip(evaluations) {
            let evaluation = evaluation?;
            processed += 1;
            if processed == 1 || processed.is_multiple_of(100) {
                eprintln!(
                    "Discovered {processed} project instance(s); {} pending",
                    queue.len()
                );
            }

            let mut evaluated = requested.clone();
            visited.insert(paths::key(&evaluated.path), evaluated.path.clone());
            if let Some(configuration) = query::property(&evaluation, "Configuration") {
                set_property(&mut evaluated.properties, "Configuration", configuration);
            }
            if let Some(platform) = query::property(&evaluation, "Platform") {
                set_property(&mut evaluated.properties, "Platform", platform);
            }
            evaluated.force_import_after_cpp_targets =
                query::property(&evaluation, "ForceImportAfterCppTargets")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .map(|path| {
                        paths::absolute_lexical(
                            &path,
                            evaluated.path.parent().unwrap_or(Path::new(".")),
                        )
                    })
                    .map(|path| {
                        if !path.is_file() {
                            bail!(
                                "{} defines missing ForceImportAfterCppTargets file {}",
                                evaluated.path.display(),
                                path.display()
                            );
                        }
                        Ok(paths::display(&path))
                    })
                    .transpose()?;
            scheduled.insert(evaluated.key());

            if extension_is(&evaluated.path, "vcxproj")
                || !query::item_groups(&evaluation, "ClCompile")?.is_empty()
            {
                native.insert(evaluated.key(), evaluated.clone());
            }

            let reference_group = if extension_is(&evaluated.path, "vcxproj") {
                "_MSBuildProjectReferenceExistent"
            } else {
                "ProjectReference"
            };
            for item in query::item_groups(&evaluation, reference_group)? {
                if query::metadata(item, "BuildReference")
                    .or_else(|| query::metadata(item, "Build"))
                    .is_some_and(|value| value.eq_ignore_ascii_case("false"))
                {
                    continue;
                }
                let full_path = query::metadata(item, "FullPath")
                    .or_else(|| query::metadata(item, "Identity"))
                    .context("ProjectReference is missing both FullPath and Identity")?;
                let reference_path = PathBuf::from(full_path);
                let reference_path = if reference_path.is_absolute() {
                    reference_path
                } else {
                    evaluated
                        .path
                        .parent()
                        .unwrap_or(Path::new("."))
                        .join(reference_path)
                };
                if !reference_path.is_file() {
                    bail!(
                        "{} references missing project {}",
                        evaluated.path.display(),
                        reference_path.display()
                    );
                }
                if !is_traversable(&reference_path) {
                    continue;
                }

                let mut properties = evaluated.properties.clone();
                for metadata_name in ["GlobalPropertiesToRemove", "UndefineProperties"] {
                    if let Some(names) = query::metadata(item, metadata_name) {
                        for name in names
                            .split(';')
                            .map(str::trim)
                            .filter(|name| !name.is_empty())
                        {
                            if let Some(existing) = properties
                                .keys()
                                .find(|candidate| candidate.eq_ignore_ascii_case(name))
                                .cloned()
                            {
                                properties.remove(&existing);
                            }
                        }
                    }
                }
                for metadata_name in [
                    "SetConfiguration",
                    "SetPlatform",
                    "SetTargetFramework",
                    "AdditionalProperties",
                ] {
                    if let Some(assignments) = query::metadata(item, metadata_name) {
                        apply_assignments(&mut properties, assignments)?;
                    }
                }

                let referenced = ProjectInstance {
                    path: paths::absolute_lexical(
                        &reference_path,
                        evaluated.path.parent().unwrap_or(Path::new(".")),
                    ),
                    properties,
                    force_import_after_cpp_targets: None,
                };
                references
                    .entry(evaluated.key())
                    .or_default()
                    .push(referenced.key());
                if scheduled.insert(referenced.key()) {
                    queue.push_back(referenced);
                }
            }
        }
    }

    let mut result: Vec<_> = native.into_values().collect();
    result.sort_by_key(ProjectInstance::key);
    let mut visited_projects: Vec<_> = visited.into_values().collect();
    visited_projects.sort_by_key(|path| paths::key(path));
    for children in references.values_mut() {
        children.sort();
        children.dedup();
    }
    Ok(Discovery {
        projects: result,
        visited_projects,
        references,
    })
}

fn evaluate_project(
    runner: &MsbuildRunner,
    project: &ProjectInstance,
) -> Result<serde_json::Value> {
    let mut arguments = property_arguments(&project.properties);
    arguments.push("-getProperty:Configuration,Platform,ForceImportAfterCppTargets".to_owned());
    if extension_is(&project.path, "vcxproj") {
        arguments.push(property_argument("BuildProjectReferences", "false"));
        arguments.push("-target:PrepareProjectReferences".to_owned());
        arguments.push("-getItem:_MSBuildProjectReferenceExistent".to_owned());
    } else {
        arguments.push("-getItem:ProjectReference,ClCompile".to_owned());
    }
    runner.query_json(
        &project.path,
        &arguments,
        &format!("Evaluating {}", project.path.display()),
    )
}

pub fn property_arguments(properties: &Properties) -> Vec<String> {
    properties
        .iter()
        .map(|(name, value)| property_argument(name, value))
        .collect()
}

pub fn property_argument(name: &str, value: &str) -> String {
    format!("-property:{name}={}", escape_property_value(value))
}

fn escape_property_value(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '%' | '*' | '?' | '@' | '$' | '(' | ')' | ';' | '\'' | ','
        ) {
            escaped.push_str(&format!("%{:02X}", character as u32));
        } else {
            escaped.push(character);
        }
    }
    escaped
}

fn apply_assignments(properties: &mut Properties, assignments: &str) -> Result<()> {
    for assignment in assignments
        .split(';')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let Some((name, value)) = assignment.split_once('=') else {
            bail!("invalid ProjectReference property assignment '{assignment}'");
        };
        set_property(properties, name.trim(), value.trim());
    }
    Ok(())
}

fn is_traversable(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.to_ascii_lowercase().ends_with("proj"))
}

fn extension_is(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_reference_property_assignments_case_insensitively() {
        let mut properties = Properties::from([("Configuration".to_owned(), "Debug".to_owned())]);
        apply_assignments(&mut properties, "Configuration=Release;Platform=x64;").unwrap();
        assert_eq!(properties.len(), 2);
        assert_eq!(
            properties
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("configuration"))
                .unwrap()
                .1,
            "Release"
        );
    }

    #[test]
    fn escapes_msbuild_property_separators_and_escape_sequences() {
        assert_eq!(
            property_argument("Probe", "a;b,c%value"),
            "-property:Probe=a%3Bb%2Cc%25value"
        );
    }
}
