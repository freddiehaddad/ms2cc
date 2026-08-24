use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use crate::{discovery::property_argument, paths};

#[derive(Debug, Clone)]
pub(super) enum PublicBmi {
    HeaderUnit { source: PathBuf, bmi: PathBuf },
    Module { name: String, bmi: PathBuf },
}

#[derive(Debug, Default)]
pub(super) struct ReferencedBmis {
    pub header_units: Vec<(PathBuf, PathBuf)>,
    pub modules: Vec<(String, PathBuf)>,
}

impl ReferencedBmis {
    pub fn deduplicate(&mut self) {
        let mut headers = HashSet::new();
        self.header_units
            .retain(|(source, bmi)| headers.insert((paths::key(source), paths::key(bmi))));
        let mut modules = HashSet::new();
        self.modules
            .retain(|(name, bmi)| modules.insert((name.to_ascii_lowercase(), paths::key(bmi))));
    }

    pub fn is_empty(&self) -> bool {
        self.header_units.is_empty() && self.modules.is_empty()
    }

    pub fn property_arguments(&self) -> Vec<String> {
        let header_units = self
            .header_units
            .iter()
            .map(|(source, bmi)| format!("{}={}", paths::display(source), paths::display(bmi)))
            .collect::<Vec<_>>();
        let header_bmis = self
            .header_units
            .iter()
            .map(|(_, bmi)| paths::display(bmi))
            .collect::<Vec<_>>();
        let modules = self
            .modules
            .iter()
            .map(|(name, bmi)| format!("{name}={}", paths::display(bmi)))
            .collect::<Vec<_>>();
        let module_bmis = self
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
}

pub(super) fn read_module_names(directory: &Path) -> Result<HashMap<String, String>> {
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
