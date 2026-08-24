use anyhow::{Context, Result, bail};
use std::{collections::HashMap, fs, path::Path};

use crate::{
    model::{ProjectInstance, Properties},
    paths,
};

use super::{
    SolutionRoots, add_solution_properties, escape_xml, get_property, is_project_path,
    set_property, set_property_if_absent,
};

pub(super) fn parse(path: &Path, base_properties: &Properties) -> Result<SolutionRoots> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read solution {}", path.display()))?;
    let mut projects = Vec::new();
    let mut solution_configurations = Vec::new();
    let mut mappings: HashMap<(String, String), ProjectMapping> = HashMap::new();
    let mut section = "";

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.starts_with("Project(") {
            let quoted = quoted_fields(line);
            if quoted.len() >= 4 {
                projects.push(SolutionProject {
                    guid: quoted[3].to_ascii_uppercase(),
                    relative_path: quoted[2].to_owned(),
                    traversable: is_project_path(&quoted[2]),
                });
            }
            continue;
        }
        if line.starts_with("GlobalSection(SolutionConfigurationPlatforms)") {
            section = "solution";
            continue;
        }
        if line.starts_with("GlobalSection(ProjectConfigurationPlatforms)") {
            section = "projects";
            continue;
        }
        if line.starts_with("EndGlobalSection") {
            section = "";
            continue;
        }
        if section == "solution"
            && let Some((left, _)) = line.split_once('=')
        {
            solution_configurations.push(left.trim().to_owned());
        } else if section == "projects"
            && let Some((left, right)) = line.split_once('=')
        {
            parse_project_mapping(left.trim(), right.trim(), &mut mappings);
        }
    }

    let selected = select_solution_configuration(base_properties, &solution_configurations)?;
    let solution_dir = path.parent().unwrap_or(Path::new("."));
    let configuration_contents =
        solution_configuration_contents(solution_dir, &projects, &mappings, &selected);
    let protected_paths = projects
        .iter()
        .filter(|project| project.traversable)
        .map(|project| {
            paths::absolute_lexical(
                &solution_dir.join(project.relative_path.replace('/', "\\")),
                solution_dir,
            )
        })
        .collect();
    let mut result = Vec::new();
    for project in projects {
        if !project.traversable {
            continue;
        }
        let mapping = mappings.get(&(project.guid.clone(), selected.to_ascii_lowercase()));
        if mappings
            .keys()
            .any(|(candidate, _)| candidate == &project.guid)
        {
            match mapping {
                Some(mapping) if mapping.build => {}
                _ => continue,
            }
        }

        let mut properties = base_properties.clone();
        if let Some(mapping) = mapping {
            set_property(&mut properties, "Configuration", &mapping.configuration);
            set_property(&mut properties, "Platform", &mapping.platform);
        } else if let Some((configuration, platform)) = selected.split_once('|') {
            set_property_if_absent(&mut properties, "Configuration", configuration);
            set_property_if_absent(&mut properties, "Platform", platform);
        }
        add_solution_properties(&mut properties, path, Some(&configuration_contents));
        let project_path = solution_dir.join(project.relative_path.replace('/', "\\"));
        if !project_path.is_file() {
            bail!(
                "solution {} references missing project {}",
                path.display(),
                project_path.display()
            );
        }
        result.push(ProjectInstance {
            path: paths::absolute_lexical(&project_path, solution_dir),
            properties,
            force_import_after_cpp_targets: None,
        });
    }
    Ok(SolutionRoots {
        projects: result,
        protected_paths,
    })
}

#[derive(Debug)]
struct SolutionProject {
    guid: String,
    relative_path: String,
    traversable: bool,
}

#[derive(Debug, Clone, Default)]
struct ProjectMapping {
    configuration: String,
    platform: String,
    build: bool,
}

fn solution_configuration_contents(
    solution_dir: &Path,
    projects: &[SolutionProject],
    mappings: &HashMap<(String, String), ProjectMapping>,
    selected: &str,
) -> String {
    let selected = selected.to_ascii_lowercase();
    let mut xml = String::from("<SolutionConfiguration>");
    for project in projects {
        let Some(mapping) = mappings.get(&(project.guid.clone(), selected.clone())) else {
            continue;
        };
        let absolute_path = paths::absolute_lexical(
            &solution_dir.join(project.relative_path.replace('/', "\\")),
            solution_dir,
        );
        xml.push_str("<ProjectConfiguration Project=\"");
        xml.push_str(&escape_xml(&project.guid));
        xml.push_str("\" AbsolutePath=\"");
        xml.push_str(&escape_xml(&paths::display(&absolute_path)));
        xml.push_str("\" BuildProjectInSolution=\"");
        xml.push_str(if mapping.build { "True" } else { "False" });
        xml.push_str("\">");
        xml.push_str(&escape_xml(&format!(
            "{}|{}",
            mapping.configuration, mapping.platform
        )));
        xml.push_str("</ProjectConfiguration>");
    }
    xml.push_str("</SolutionConfiguration>");
    xml
}

fn parse_project_mapping(
    left: &str,
    right: &str,
    mappings: &mut HashMap<(String, String), ProjectMapping>,
) {
    let Some(close) = left.find('}') else {
        return;
    };
    let guid = left[..=close].to_ascii_uppercase();
    let suffix = left[close + 1..].trim_start_matches('.');
    let (solution_configuration, kind) = if let Some(value) = suffix.strip_suffix(".ActiveCfg") {
        (value, "active")
    } else if let Some(value) = suffix.strip_suffix(".Build.0") {
        (value, "build")
    } else {
        return;
    };
    let entry = mappings
        .entry((guid, solution_configuration.to_ascii_lowercase()))
        .or_default();
    if kind == "build" {
        entry.build = true;
    }
    if let Some((configuration, platform)) = right.split_once('|') {
        entry.configuration = configuration.to_owned();
        entry.platform = platform.to_owned();
    }
}

fn select_solution_configuration(
    properties: &Properties,
    configurations: &[String],
) -> Result<String> {
    let requested_configuration = get_property(properties, "Configuration");
    let requested_platform = get_property(properties, "Platform");
    if let (Some(configuration), Some(platform)) = (requested_configuration, requested_platform) {
        let selected = format!("{configuration}|{platform}");
        if configurations.is_empty()
            || configurations
                .iter()
                .any(|value| value.eq_ignore_ascii_case(&selected))
        {
            return Ok(selected);
        }
        bail!(
            "solution does not define configuration/platform '{}'; available values: {}",
            selected,
            configurations.join(", ")
        );
    }

    let candidates: Vec<_> = configurations
        .iter()
        .filter(|value| {
            let (configuration, platform) = value.split_once('|').unwrap_or((value.as_str(), ""));
            requested_configuration
                .is_none_or(|requested| requested.eq_ignore_ascii_case(configuration))
                && requested_platform
                    .is_none_or(|requested| requested.eq_ignore_ascii_case(platform))
        })
        .collect();
    match candidates.as_slice() {
        [only] => Ok((*only).clone()),
        [] if configurations.is_empty() => Ok(format!(
            "{}|{}",
            requested_configuration.unwrap_or(""),
            requested_platform.unwrap_or("")
        )),
        [] => bail!(
            "no solution configuration matches the requested configuration/platform; available values: {}",
            configurations.join(", ")
        ),
        _ => bail!(
            "solution configuration is ambiguous; pass --configuration and --platform (available: {})",
            candidates
                .iter()
                .map(|value| value.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn quoted_fields(line: &str) -> Vec<String> {
    line.split('"')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, value)| value.to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_quoted_solution_fields() {
        let fields =
            quoted_fields(r#"Project("{TYPE}") = "App", "src\App\App.vcxproj", "{PROJECT}""#);
        assert_eq!(
            fields,
            vec!["{TYPE}", "App", r"src\App\App.vcxproj", "{PROJECT}"]
        );
    }
}
