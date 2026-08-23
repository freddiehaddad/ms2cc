use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::paths;

pub type Properties = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInstance {
    pub path: PathBuf,
    pub properties: Properties,
    pub force_import_after_cpp_targets: Option<String>,
}

impl ProjectInstance {
    pub fn key(&self) -> String {
        let mut key = self
            .path
            .to_string_lossy()
            .replace('/', "\\")
            .to_lowercase();
        let properties = property_key(&self.properties);
        if !properties.is_empty() {
            key.push('\0');
            key.push_str(&properties);
        }
        key
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompileCommand {
    pub directory: String,
    pub arguments: Vec<String>,
    pub file: String,
}

#[derive(Debug, Clone)]
pub struct CommandRecord {
    pub command: CompileCommand,
    pub project: PathBuf,
    pub properties: Properties,
}

impl CommandRecord {
    pub fn source_key(&self) -> SourceKey {
        SourceKey {
            project: paths::key(&self.project),
            properties: property_key(&self.properties),
            source: paths::key(Path::new(&self.command.file)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourceKey {
    pub project: String,
    pub properties: String,
    pub source: String,
}

impl SourceKey {
    pub fn new(project: &Path, properties: &Properties, source: &Path) -> Self {
        Self {
            project: paths::key(project),
            properties: property_key(properties),
            source: paths::key(source),
        }
    }
}

fn property_key(properties: &Properties) -> String {
    let mut entries: Vec<_> = properties
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value))
        .collect();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("\0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_keys_normalize_property_name_case_before_sorting() {
        let left = ProjectInstance {
            path: PathBuf::from(r"C:\project\app.vcxproj"),
            properties: Properties::from([
                ("Zeta".to_owned(), "one".to_owned()),
                ("alpha".to_owned(), "two".to_owned()),
            ]),
            force_import_after_cpp_targets: None,
        };
        let right = ProjectInstance {
            path: PathBuf::from(r"c:\PROJECT\app.vcxproj"),
            properties: Properties::from([
                ("Alpha".to_owned(), "two".to_owned()),
                ("zeta".to_owned(), "one".to_owned()),
            ]),
            force_import_after_cpp_targets: None,
        };

        assert_eq!(left.key(), right.key());
    }
}
