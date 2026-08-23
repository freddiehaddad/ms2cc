use anyhow::{Result, bail};
use clap::Parser;
use std::path::{Path, PathBuf};

use crate::paths;

const PACKAGE_DESCRIPTION: &str = env!("CARGO_PKG_DESCRIPTION");

#[derive(Debug, Clone)]
pub struct Property {
    pub name: String,
    pub value: String,
}

fn parse_property(value: &str) -> Result<Property, String> {
    let Some((name, property_value)) = value.split_once('=') else {
        return Err("expected NAME=VALUE".to_owned());
    };
    if name.is_empty() {
        return Err("property name cannot be empty".to_owned());
    }
    if !name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '.'))
    {
        return Err(format!("invalid MSBuild property name '{name}'"));
    }
    Ok(Property {
        name: name.to_owned(),
        value: property_value.to_owned(),
    })
}

#[derive(Debug, Parser)]
#[command(version, about = PACKAGE_DESCRIPTION)]
pub struct Args {
    /// Visual C++ project, solution, or traversal project roots
    #[arg(required = true, value_name = "PROJECT_OR_SOLUTION")]
    pub inputs: Vec<PathBuf>,

    /// Output compilation database
    #[arg(short = 'o', long, default_value = "compile_commands.json")]
    pub output: PathBuf,

    /// MSBuild configuration to evaluate
    #[arg(short = 'c', long)]
    pub configuration: Option<String>,

    /// MSBuild platform to evaluate
    #[arg(long)]
    pub platform: Option<String>,

    /// Additional MSBuild property
    #[arg(long = "property", value_name = "NAME=VALUE", value_parser = parse_property)]
    pub properties: Vec<Property>,

    /// MSBuild executable or command wrapper
    #[arg(long)]
    pub msbuild: Option<PathBuf>,

    /// Maximum seconds without process completion
    #[arg(
        long,
        default_value_t = 600,
        value_name = "SECONDS",
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    pub timeout: u64,
}

impl Args {
    pub fn resolve_inputs(&self, cwd: &Path) -> Result<Vec<PathBuf>> {
        let mut resolved = Vec::with_capacity(self.inputs.len());
        for input in &self.inputs {
            let absolute = if input.is_absolute() {
                input.clone()
            } else {
                cwd.join(input)
            };
            if !absolute.is_file() {
                bail!(
                    "input does not exist or is not a file: {}",
                    absolute.display()
                );
            }
            let extension = absolute
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if !matches!(
                extension.to_ascii_lowercase().as_str(),
                "vcxproj" | "proj" | "sln" | "slnx"
            ) {
                bail!(
                    "unsupported input '{}'; expected .vcxproj, .proj, .sln, or .slnx",
                    absolute.display()
                );
            }
            resolved.push(paths::absolute_lexical(&absolute, cwd));
        }
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_requires_name_and_value() {
        assert!(parse_property("ProductEdition=Enterprise").is_ok());
        assert!(parse_property("ProductEdition").is_err());
        assert!(parse_property("bad-name=value").is_err());
    }
}
