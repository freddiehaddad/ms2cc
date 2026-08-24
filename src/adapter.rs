use anyhow::{Context, Result};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use crate::{model::ProjectInstance, paths};

const ADAPTER: &str = include_str!("adapter/ms2cc.targets");
const ORIGINAL_IMPORTS_MARKER: &str = "<!-- MS2CC_ORIGINAL_IMPORTS -->";

pub fn write(directory: &Path, projects: &[ProjectInstance]) -> Result<PathBuf> {
    let path = directory.join("ms2cc.targets");
    let mut originals: BTreeMap<String, Vec<&ProjectInstance>> = BTreeMap::new();
    for project in projects {
        if project.force_import_after_cpp_targets.is_some() {
            originals
                .entry(paths::key(&project.path))
                .or_default()
                .push(project);
        }
    }
    let imports = originals
        .into_values()
        .flat_map(|instances| {
            let distinguish_instances = instances.len() > 1;
            let property_names: Vec<_> = instances
                .iter()
                .flat_map(|instance| instance.properties.keys())
                .fold(BTreeMap::new(), |mut names, name| {
                    names
                        .entry(name.to_ascii_lowercase())
                        .or_insert_with(|| name.clone());
                    names
                })
                .into_values()
                .collect();
            instances.into_iter().filter_map(move |instance| {
                let original = instance.force_import_after_cpp_targets.as_deref()?;
                let mut condition = format!(
                    "'$(MSBuildProjectFullPath)' == '{}'",
                    escape_msbuild_condition_literal(&paths::display(&instance.path))
                );
                if distinguish_instances {
                    for name in &property_names {
                        let value = instance
                            .properties
                            .iter()
                            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
                            .map(|(_, value)| value.as_str())
                            .unwrap_or_default();
                        condition.push_str(&format!(
                            " And '$({name})' == '{}'",
                            escape_msbuild_condition_literal(value)
                        ));
                    }
                }
                condition.push_str(&format!(
                    " And Exists('{}')",
                    escape_msbuild_condition_literal(original)
                ));
                Some(format!(
                    "  <Import Project=\"{}\" Condition=\"{}\" />",
                    escape_xml(original),
                    escape_xml(&condition)
                ))
            })
        })
        .collect::<Vec<_>>()
        .join("\r\n");
    let contents = ADAPTER.replace(ORIGINAL_IMPORTS_MARKER, &imports);
    fs::write(&path, contents)
        .with_context(|| format!("failed to write temporary adapter {}", path.display()))?;
    Ok(path)
}

fn escape_msbuild_condition_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '%' | '$' | '@' | '(' | ')' | ';' | '\'') {
            escaped.push_str(&format!("%{:02X}", character as u32));
        } else {
            escaped.push(character);
        }
    }
    escaped
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Properties;
    use tempfile::TempDir;

    #[test]
    fn force_import_dispatch_distinguishes_absent_instance_properties() {
        let directory = TempDir::new().unwrap();
        let project_path = PathBuf::from(r"C:\project\app.vcxproj");
        let projects = vec![
            ProjectInstance {
                path: project_path.clone(),
                properties: Properties::from([("Configuration".to_owned(), "Release".to_owned())]),
                force_import_after_cpp_targets: Some(r"C:\project\release.targets".to_owned()),
            },
            ProjectInstance {
                path: project_path,
                properties: Properties::from([
                    ("Configuration".to_owned(), "Release".to_owned()),
                    ("Flavor".to_owned(), "Special".to_owned()),
                ]),
                force_import_after_cpp_targets: Some(r"C:\project\special.targets".to_owned()),
            },
        ];

        let contents = fs::read_to_string(write(directory.path(), &projects).unwrap()).unwrap();
        assert!(contents.contains(
            "release.targets\" Condition=\"'$(MSBuildProjectFullPath)' == 'C:\\project\\app.vcxproj' And '$(Configuration)' == 'Release' And '$(Flavor)' == ''"
        ));
        assert!(contents.contains(
            "special.targets\" Condition=\"'$(MSBuildProjectFullPath)' == 'C:\\project\\app.vcxproj' And '$(Configuration)' == 'Release' And '$(Flavor)' == 'Special'"
        ));
    }
}
