mod sln;
mod slnx;

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::{
    model::{ProjectInstance, Properties},
    paths,
};

#[derive(Debug)]
pub struct SolutionRoots {
    pub projects: Vec<ProjectInstance>,
    pub protected_paths: Vec<PathBuf>,
}

pub fn projects(path: &Path, base_properties: &Properties) -> Result<SolutionRoots> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "sln" => sln::parse(path, base_properties),
        "slnx" => slnx::parse(path, base_properties),
        _ => Ok(SolutionRoots {
            projects: vec![ProjectInstance {
                path: path.to_path_buf(),
                properties: base_properties.clone(),
                force_import_after_cpp_targets: None,
            }],
            protected_paths: vec![path.to_path_buf()],
        }),
    }
}

fn add_solution_properties(
    properties: &mut Properties,
    solution_path: &Path,
    configuration_contents: Option<&str>,
) {
    let absolute_path = paths::absolute_lexical(
        solution_path,
        solution_path.parent().unwrap_or(Path::new(".")),
    );
    let mut directory = paths::display(absolute_path.parent().unwrap_or(Path::new(".")));
    if !directory.ends_with('\\') {
        directory.push('\\');
    }
    set_property(properties, "BuildingSolutionFile", "true");
    set_property(properties, "SolutionDir", &directory);
    set_property(properties, "SolutionPath", &paths::display(&absolute_path));
    set_property(
        properties,
        "SolutionName",
        absolute_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default(),
    );
    set_property(
        properties,
        "SolutionFileName",
        absolute_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default(),
    );
    set_property(
        properties,
        "SolutionExt",
        absolute_path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| format!(".{value}"))
            .as_deref()
            .unwrap_or_default(),
    );
    if let Some(contents) = configuration_contents {
        set_property(properties, "CurrentSolutionConfigurationContents", contents);
    }
}

fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn is_project_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.to_ascii_lowercase().ends_with("proj"))
}

pub fn set_property(properties: &mut Properties, name: &str, value: &str) {
    if let Some(existing) = properties
        .keys()
        .find(|candidate| candidate.eq_ignore_ascii_case(name))
        .cloned()
    {
        properties.remove(&existing);
    }
    properties.insert(name.to_owned(), value.to_owned());
}

fn set_property_if_absent(properties: &mut Properties, name: &str, value: &str) {
    if get_property(properties, name).is_none() && !value.is_empty() {
        properties.insert(name.to_owned(), value.to_owned());
    }
}

fn get_property<'a>(properties: &'a Properties, name: &str) -> Option<&'a str> {
    properties
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn property_names_are_case_insensitive() {
        let mut properties = BTreeMap::from([("Configuration".to_owned(), "Debug".to_owned())]);
        set_property(&mut properties, "configuration", "Release");
        assert_eq!(properties.len(), 1);
        assert_eq!(get_property(&properties, "CONFIGURATION"), Some("Release"));
    }
}
