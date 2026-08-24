use anyhow::{Context, Result, bail};
use quick_xml::{Reader, XmlVersion, events::Event};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use crate::{
    model::{ProjectInstance, Properties},
    paths,
};

use super::{
    SolutionRoots, add_solution_properties, escape_xml, get_property, is_project_path, set_property,
};

pub(super) fn parse(path: &Path, base_properties: &Properties) -> Result<SolutionRoots> {
    let mut reader = Reader::from_file(path)
        .with_context(|| format!("failed to read solution {}", path.display()))?;
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut parser = SlnxParser::default();

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => {
                let name = event.name().as_ref().to_owned();
                let attributes = xml_attributes(&event)?;
                parser.process_element(&name, &attributes, false)?;
                parser.stack.push(name);
            }
            Ok(Event::Empty(event)) => {
                let name = event.name().as_ref().to_owned();
                let attributes = xml_attributes(&event)?;
                parser.process_element(&name, &attributes, true)?;
            }
            Ok(Event::End(event)) => {
                let name = event.name().as_ref().to_owned();
                parser.finish_element(&name);
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => {
                bail!("failed to parse solution {}: {error}", path.display());
            }
        }
        buffer.clear();
    }

    let directory = path.parent().unwrap_or(Path::new("."));
    let (solution_build_type, solution_platform) =
        select_slnx_configuration(base_properties, &parser.build_types, &parser.platforms)?;
    let mut resolved = Vec::new();
    for project in parser.projects {
        if !is_project_path(&project.path) {
            continue;
        }
        let extension = Path::new(&project.path)
            .extension()
            .and_then(|value| value.to_str());
        let has_custom_type = find_project_type(
            &parser.project_types,
            project.project_type.as_deref(),
            extension,
        )
        .is_some();
        let mut rules =
            if !has_custom_type && project.path.to_ascii_lowercase().ends_with(".vcxproj") {
                builtin_vcxproj_rules()
            } else {
                Vec::new()
            };
        append_project_type_rules(
            &mut rules,
            &parser.project_types,
            project.project_type.as_deref(),
            extension,
            &mut HashSet::new(),
        );
        append_default_project_type_rules(&mut rules, &parser.project_types, &mut HashSet::new());
        rules.extend(project.rules);

        let build = rule_value(
            &rules,
            SlnxDimension::Build,
            &solution_build_type,
            &solution_platform,
        )
        .map(|value| value.parse::<bool>())
        .transpose()
        .context("invalid boolean in .slnx Build rule")?
        .unwrap_or(true);
        let project_build_type = mapped_dimension(
            &rules,
            SlnxDimension::BuildType,
            &solution_build_type,
            &solution_platform,
        )
        .unwrap_or_else(|| solution_build_type.clone());
        let mut project_platform = mapped_dimension(
            &rules,
            SlnxDimension::Platform,
            &solution_build_type,
            &solution_platform,
        )
        .unwrap_or_else(|| solution_platform.clone());
        if project_platform == "?" {
            project_platform.clear();
        }

        let candidate = directory.join(project.path.replace('/', "\\"));
        if !candidate.is_file() {
            bail!(
                "solution {} references missing project {}",
                path.display(),
                candidate.display()
            );
        }
        let canonical = paths::absolute_lexical(&candidate, directory);
        resolved.push(ResolvedSlnxProject {
            path: canonical,
            configuration: project_build_type,
            platform: project_platform,
            build,
        });
    }

    let configuration_contents = solution_configuration_contents(&resolved);
    let protected_paths = resolved
        .iter()
        .map(|project| project.path.clone())
        .collect();
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for project in resolved {
        if !project.build {
            continue;
        }
        let canonical = project.path;
        if seen.insert(canonical.to_string_lossy().to_ascii_lowercase()) {
            let mut properties = base_properties.clone();
            if !project.configuration.is_empty() {
                set_property(&mut properties, "Configuration", &project.configuration);
            }
            if !project.platform.is_empty() {
                set_property(&mut properties, "Platform", &project.platform);
            }
            add_solution_properties(&mut properties, path, Some(&configuration_contents));
            result.push(ProjectInstance {
                path: canonical,
                properties,
                force_import_after_cpp_targets: None,
            });
        }
    }
    Ok(SolutionRoots {
        projects: result,
        protected_paths,
    })
}

#[derive(Debug)]
struct ResolvedSlnxProject {
    path: PathBuf,
    configuration: String,
    platform: String,
    build: bool,
}

fn solution_configuration_contents(projects: &[ResolvedSlnxProject]) -> String {
    let mut xml = String::from("<SolutionConfiguration>");
    for (index, project) in projects.iter().enumerate() {
        xml.push_str("<ProjectConfiguration Project=\"{00000000-0000-0000-0000-");
        xml.push_str(&format!("{:012X}", index + 1));
        xml.push_str("}\" AbsolutePath=\"");
        xml.push_str(&escape_xml(&paths::display(&project.path)));
        xml.push_str("\" BuildProjectInSolution=\"");
        xml.push_str(if project.build { "True" } else { "False" });
        xml.push_str("\">");
        xml.push_str(&escape_xml(&format_project_configuration(
            &project.configuration,
            &project.platform,
        )));
        xml.push_str("</ProjectConfiguration>");
    }
    xml.push_str("</SolutionConfiguration>");
    xml
}

#[derive(Default)]
struct SlnxParser {
    stack: Vec<String>,
    build_types: Vec<String>,
    platforms: Vec<String>,
    project_types: Vec<SlnxProjectType>,
    projects: Vec<SlnxProject>,
    current_project_type: Option<SlnxProjectType>,
    current_project: Option<SlnxProject>,
}

impl SlnxParser {
    fn process_element(
        &mut self,
        name: &str,
        attributes: &HashMap<String, String>,
        empty: bool,
    ) -> Result<()> {
        let parent = self.stack.last().map(String::as_str);
        if name.eq_ignore_ascii_case("ProjectType")
            && parent.is_some_and(|value| value.eq_ignore_ascii_case("Configurations"))
        {
            self.current_project_type = Some(SlnxProjectType::from_attributes(attributes)?);
            if empty && let Some(project_type) = self.current_project_type.take() {
                self.project_types.push(project_type);
            }
        } else if name.eq_ignore_ascii_case("Project") {
            self.current_project = Some(SlnxProject::from_attributes(attributes)?);
            if empty && let Some(project) = self.current_project.take() {
                self.projects.push(project);
            }
        } else if self.current_project_type.is_none()
            && self.current_project.is_none()
            && parent.is_some_and(|value| value.eq_ignore_ascii_case("Configurations"))
            && name.eq_ignore_ascii_case("BuildType")
        {
            push_named_dimension(&mut self.build_types, attributes, "BuildType")?;
        } else if self.current_project_type.is_none()
            && self.current_project.is_none()
            && parent.is_some_and(|value| value.eq_ignore_ascii_case("Configurations"))
            && name.eq_ignore_ascii_case("Platform")
        {
            push_named_dimension(&mut self.platforms, attributes, "Platform")?;
        } else if let Some(rule) = slnx_rule(name, attributes)? {
            if let Some(project) = &mut self.current_project {
                project.rules.push(rule);
            } else if let Some(project_type) = &mut self.current_project_type {
                project_type.rules.push(rule);
            }
        }
        Ok(())
    }

    fn finish_element(&mut self, name: &str) {
        if name.eq_ignore_ascii_case("Project") {
            if let Some(project) = self.current_project.take() {
                self.projects.push(project);
            }
        } else if name.eq_ignore_ascii_case("ProjectType")
            && let Some(project_type) = self.current_project_type.take()
        {
            self.project_types.push(project_type);
        }
        self.stack.pop();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlnxDimension {
    BuildType,
    Platform,
    Build,
    Deploy,
}

#[derive(Debug, Clone)]
struct SlnxRule {
    dimension: SlnxDimension,
    solution_build_type: String,
    solution_platform: String,
    project_value: String,
}

#[derive(Debug)]
struct SlnxProject {
    path: String,
    project_type: Option<String>,
    rules: Vec<SlnxRule>,
}

impl SlnxProject {
    fn from_attributes(attributes: &HashMap<String, String>) -> Result<Self> {
        Ok(Self {
            path: required_attribute(attributes, "Path", "Project")?.to_owned(),
            project_type: attribute(attributes, "Type").map(str::to_owned),
            rules: Vec::new(),
        })
    }
}

#[derive(Debug)]
struct SlnxProjectType {
    type_id: Option<String>,
    name: Option<String>,
    extension: Option<String>,
    based_on: Option<String>,
    is_buildable: bool,
    supports_platform: bool,
    rules: Vec<SlnxRule>,
}

impl SlnxProjectType {
    fn from_attributes(attributes: &HashMap<String, String>) -> Result<Self> {
        Ok(Self {
            type_id: attribute(attributes, "TypeId").map(str::to_owned),
            name: attribute(attributes, "Name").map(str::to_owned),
            extension: attribute(attributes, "Extension").map(str::to_owned),
            based_on: attribute(attributes, "BasedOn").map(str::to_owned),
            is_buildable: attribute(attributes, "IsBuildable")
                .map(str::parse)
                .transpose()
                .context("invalid .slnx ProjectType IsBuildable value")?
                .unwrap_or(true),
            supports_platform: attribute(attributes, "SupportsPlatform")
                .map(str::parse)
                .transpose()
                .context("invalid .slnx ProjectType SupportsPlatform value")?
                .unwrap_or(true),
            rules: Vec::new(),
        })
    }

    fn matches(&self, name: Option<&str>, extension: Option<&str>) -> bool {
        name.is_some_and(|name| {
            self.type_id
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case(name))
                || self
                    .name
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case(name))
        }) || name.is_none()
            && extension.is_some_and(|extension| {
                self.extension.as_deref().is_some_and(|value| {
                    value
                        .trim_start_matches('.')
                        .eq_ignore_ascii_case(extension.trim_start_matches('.'))
                })
            })
    }

    fn is_default(&self) -> bool {
        self.type_id.is_none() && self.name.is_none() && self.extension.is_none()
    }
}

fn xml_attributes(event: &quick_xml::events::BytesStart<'_>) -> Result<HashMap<String, String>> {
    let mut result = HashMap::new();
    for attribute in event.attributes() {
        let attribute = attribute.context("invalid .slnx attribute")?;
        let name = attribute.key.as_ref().to_owned();
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .context("invalid .slnx attribute value")?
            .into_owned();
        result.insert(name, value);
    }
    Ok(result)
}

fn attribute<'a>(attributes: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn required_attribute<'a>(
    attributes: &'a HashMap<String, String>,
    name: &str,
    element: &str,
) -> Result<&'a str> {
    attribute(attributes, name)
        .with_context(|| format!(".slnx {element} element is missing {name}"))
}

fn push_named_dimension(
    values: &mut Vec<String>,
    attributes: &HashMap<String, String>,
    element: &str,
) -> Result<()> {
    let value = required_attribute(attributes, "Name", element)?;
    if !values.iter().any(|item| item.eq_ignore_ascii_case(value)) {
        values.push(value.to_owned());
    }
    Ok(())
}

fn slnx_rule(name: &str, attributes: &HashMap<String, String>) -> Result<Option<SlnxRule>> {
    let dimension = if name.eq_ignore_ascii_case("BuildType") {
        SlnxDimension::BuildType
    } else if name.eq_ignore_ascii_case("Platform") {
        SlnxDimension::Platform
    } else if name.eq_ignore_ascii_case("Build") {
        SlnxDimension::Build
    } else if name.eq_ignore_ascii_case("Deploy") {
        SlnxDimension::Deploy
    } else {
        return Ok(None);
    };
    let (solution_build_type, solution_platform) =
        parse_slnx_rule_scope(attribute(attributes, "Solution").unwrap_or_default())?;
    let project_value = attribute(attributes, "Project")
        .map(str::to_owned)
        .or_else(|| {
            matches!(dimension, SlnxDimension::Build | SlnxDimension::Deploy)
                .then(|| "true".to_owned())
        })
        .with_context(|| format!(".slnx {name} rule is missing Project"))?;
    Ok(Some(SlnxRule {
        dimension,
        solution_build_type,
        solution_platform,
        project_value,
    }))
}

fn parse_slnx_rule_scope(value: &str) -> Result<(String, String)> {
    if value.is_empty() {
        return Ok((String::new(), String::new()));
    }
    let (build_type, platform) = value
        .split_once('|')
        .with_context(|| format!("invalid .slnx configuration rule scope '{value}'"))?;
    if build_type.is_empty() || platform.is_empty() {
        bail!("invalid .slnx configuration rule scope '{value}'");
    }
    Ok((
        if build_type != "*" {
            build_type.to_owned()
        } else {
            String::new()
        },
        if platform != "*" {
            platform.to_owned()
        } else {
            String::new()
        },
    ))
}

fn select_slnx_configuration(
    properties: &Properties,
    build_types: &[String],
    platforms: &[String],
) -> Result<(String, String)> {
    Ok((
        select_slnx_dimension(
            get_property(properties, "Configuration"),
            build_types,
            "configuration",
        )?,
        select_slnx_dimension(get_property(properties, "Platform"), platforms, "platform")?,
    ))
}

fn select_slnx_dimension(
    requested: Option<&str>,
    available: &[String],
    name: &str,
) -> Result<String> {
    if let Some(requested) = requested {
        if available.is_empty() {
            return Ok(requested.to_owned());
        }
        if let Some(value) = available.iter().find(|value| {
            if name == "platform" {
                canonical_platform(value) == canonical_platform(requested)
            } else {
                value.eq_ignore_ascii_case(requested)
            }
        }) {
            return Ok(value.clone());
        }
        bail!(
            ".slnx does not define {name} '{requested}'; available values: {}",
            available.join(", ")
        );
    }
    match available {
        [] => Ok(String::new()),
        [only] => Ok(only.clone()),
        _ => bail!(
            ".slnx {name} is ambiguous; specify it explicitly (available: {})",
            available.join(", ")
        ),
    }
}

fn builtin_vcxproj_rules() -> Vec<SlnxRule> {
    vec![
        SlnxRule {
            dimension: SlnxDimension::Platform,
            solution_build_type: String::new(),
            solution_platform: "Any CPU".to_owned(),
            project_value: "x64".to_owned(),
        },
        SlnxRule {
            dimension: SlnxDimension::Platform,
            solution_build_type: String::new(),
            solution_platform: "x86".to_owned(),
            project_value: "Win32".to_owned(),
        },
    ]
}

fn append_project_type_rules(
    destination: &mut Vec<SlnxRule>,
    project_types: &[SlnxProjectType],
    project_type_name: Option<&str>,
    extension: Option<&str>,
    active: &mut HashSet<String>,
) {
    let Some(project_type) = find_project_type(project_types, project_type_name, extension) else {
        return;
    };
    append_project_type_definition(destination, project_types, project_type, active);
}

fn find_project_type<'a>(
    project_types: &'a [SlnxProjectType],
    project_type_name: Option<&str>,
    extension: Option<&str>,
) -> Option<&'a SlnxProjectType> {
    project_types
        .iter()
        .rev()
        .find(|project_type| project_type.matches(project_type_name, extension))
}

fn append_default_project_type_rules(
    destination: &mut Vec<SlnxRule>,
    project_types: &[SlnxProjectType],
    active: &mut HashSet<String>,
) {
    if let Some(project_type) = project_types
        .iter()
        .rev()
        .find(|project_type| project_type.is_default())
    {
        append_project_type_definition(destination, project_types, project_type, active);
    }
}

fn append_project_type_definition(
    destination: &mut Vec<SlnxRule>,
    project_types: &[SlnxProjectType],
    project_type: &SlnxProjectType,
    active: &mut HashSet<String>,
) {
    let key = project_type
        .type_id
        .as_deref()
        .or(project_type.name.as_deref())
        .or(project_type.extension.as_deref())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !active.insert(key.clone()) {
        return;
    }
    if let Some(based_on) = &project_type.based_on {
        if is_builtin_vcxproj_type(based_on) {
            destination.extend(builtin_vcxproj_rules());
        } else {
            append_project_type_rules(destination, project_types, Some(based_on), None, active);
        }
    }
    if !project_type.is_buildable {
        destination.push(SlnxRule {
            dimension: SlnxDimension::Build,
            solution_build_type: String::new(),
            solution_platform: String::new(),
            project_value: "false".to_owned(),
        });
    }
    if !project_type.supports_platform {
        destination.push(SlnxRule {
            dimension: SlnxDimension::Platform,
            solution_build_type: String::new(),
            solution_platform: String::new(),
            project_value: "?".to_owned(),
        });
    }
    destination.extend(project_type.rules.iter().cloned());
    active.remove(&key);
}

fn is_builtin_vcxproj_type(value: &str) -> bool {
    value.eq_ignore_ascii_case("VC")
        || value.eq_ignore_ascii_case("vcxproj")
        || value.eq_ignore_ascii_case(".vcxproj")
        || value
            .trim_matches(['{', '}'])
            .eq_ignore_ascii_case("8BC9CEB8-8B4A-11D0-8D11-00A0C91BC942")
}

fn format_project_configuration(configuration: &str, platform: &str) -> String {
    if platform.is_empty() {
        configuration.to_owned()
    } else {
        format!("{configuration}|{platform}")
    }
}

fn mapped_dimension(
    rules: &[SlnxRule],
    dimension: SlnxDimension,
    solution_build_type: &str,
    solution_platform: &str,
) -> Option<String> {
    rule_value(rules, dimension, solution_build_type, solution_platform).map(|value| {
        if value == "*" {
            match dimension {
                SlnxDimension::BuildType => solution_build_type,
                SlnxDimension::Platform => solution_platform,
                _ => value,
            }
            .to_owned()
        } else {
            value.to_owned()
        }
    })
}

fn rule_value<'a>(
    rules: &'a [SlnxRule],
    dimension: SlnxDimension,
    solution_build_type: &str,
    solution_platform: &str,
) -> Option<&'a str> {
    rules
        .iter()
        .rev()
        .find(|rule| {
            rule.dimension == dimension
                && (rule.solution_build_type.is_empty()
                    || rule
                        .solution_build_type
                        .eq_ignore_ascii_case(solution_build_type))
                && (rule.solution_platform.is_empty()
                    || canonical_platform(&rule.solution_platform)
                        == canonical_platform(solution_platform))
        })
        .map(|rule| rule.project_value.as_str())
}

fn canonical_platform(value: &str) -> String {
    if value.eq_ignore_ascii_case("Any CPU") || value.eq_ignore_ascii_case("AnyCPU") {
        "anycpu".to_owned()
    } else {
        value.to_ascii_lowercase()
    }
}
