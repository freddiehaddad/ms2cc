mod adapter;
mod cli;
mod command_line;
mod discovery;
mod extractor;
mod model;
mod msbuild;
mod output;
mod paths;
mod query;
mod solution;

use anyhow::Result;
use clap::Parser;
use cli::Args;

fn main() -> Result<()> {
    let args = Args::parse();
    ms2cc::run(args)
}

mod ms2cc {
    use super::{
        adapter, cli::Args, discovery, extractor, model::Properties, msbuild::MsbuildRunner,
        output, paths,
    };
    use anyhow::{Context, Result, bail};
    use rayon::ThreadPoolBuilder;
    use std::{
        collections::BTreeMap,
        env,
        path::{Path, PathBuf},
        time::Duration,
    };
    use tempfile::TempDir;

    pub fn run(args: Args) -> Result<()> {
        let cwd = env::current_dir().context("failed to determine the current directory")?;
        let inputs = args.resolve_inputs(&cwd)?;
        let output_path = paths::absolute_lexical(&args.output, &cwd);
        reject_output_collision(
            &output_path,
            inputs.iter().map(PathBuf::as_path),
            "an input project or solution",
        )?;
        let properties = build_properties(&args)?;
        let temp_dir = TempDir::new().context("failed to create the ms2cc temporary directory")?;
        let runner = MsbuildRunner::discover(
            args.msbuild.as_deref(),
            cwd,
            temp_dir.path().to_path_buf(),
            Duration::from_secs(args.timeout),
        )?;

        runner.require_query_support()?;

        let worker_count = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .clamp(1, 8);
        let pool = ThreadPoolBuilder::new()
            .num_threads(worker_count)
            .thread_name(|index| format!("ms2cc-{index}"))
            .build()
            .context("failed to initialize the extraction worker pool")?;

        eprintln!(
            "Discovering project graph with {} (up to {} workers)",
            runner.display_name(),
            worker_count
        );
        let discovery = pool.install(|| discovery::discover(&runner, &inputs, &properties))?;
        let projects = discovery.projects;
        if projects.is_empty() {
            bail!("no Visual C++ projects were found in the requested input scope");
        }
        reject_output_collision(
            &output_path,
            discovery.visited_projects.iter().map(PathBuf::as_path),
            "discovered project",
        )?;
        let adapter_path = adapter::write(temp_dir.path(), &projects)?;

        eprintln!(
            "Extracting {} Visual C++ project instance(s)",
            projects.len()
        );
        let extraction = pool.install(|| {
            extractor::extract_all(&runner, &adapter_path, &projects, &discovery.references)
        })?;
        extraction.prove_complete()?;
        let expected_source_count = extraction.expected_source_count();
        reject_output_collision(
            &output_path,
            extraction
                .commands
                .iter()
                .map(|record| Path::new(&record.command.file)),
            "source file",
        )?;

        let commands = output::prepare(extraction.commands)?;
        if commands.is_empty() {
            bail!("MSBuild returned no C/C++ compilation commands");
        }

        fn reject_output_collision<'a>(
            output: &Path,
            candidates: impl IntoIterator<Item = &'a Path>,
            kind: &str,
        ) -> Result<()> {
            let output_identity = path_identity(output);
            if let Some(candidate) = candidates
                .into_iter()
                .find(|candidate| path_identity(candidate) == output_identity)
            {
                bail!(
                    "output path cannot overwrite {kind} {}",
                    candidate.display()
                );
            }
            Ok(())
        }

        fn path_identity(path: &Path) -> String {
            if let Ok(canonical) = path.canonicalize() {
                return paths::key(&canonical);
            }
            if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
                && let Ok(canonical_parent) = parent.canonicalize()
            {
                return paths::key(&canonical_parent.join(name));
            }
            paths::key(path)
        }

        output::write_atomic(&output_path, &commands)?;
        eprintln!(
            "Wrote {} compilation command(s) for {} expected source item(s) from {} project instance(s) to {}",
            commands.len(),
            expected_source_count,
            projects.len(),
            output_path.display()
        );
        Ok(())
    }

    fn build_properties(args: &Args) -> Result<Properties> {
        let mut properties = BTreeMap::new();
        if let Some(configuration) = &args.configuration {
            insert_property(&mut properties, "Configuration", configuration)?;
        }
        if let Some(platform) = &args.platform {
            insert_property(&mut properties, "Platform", platform)?;
        }
        for property in &args.properties {
            if [
                "DesignTimeBuild",
                "BuildingInsideVisualStudio",
                "BuildProjectReferences",
                "BuildingProject",
                "AlwaysResolveReferencesDesignTime",
                "ShouldUnsetParentConfigurationAndPlatform",
                "ForceImportAfterCppTargets",
                "Ms2ccScratchDirectory",
                "Ms2ccReferencedHeaderUnits",
                "Ms2ccReferencedHeaderUnitBMIs",
                "Ms2ccReferencedModules",
                "Ms2ccReferencedModuleBMIs",
                "_FindDependencies",
                "FindDependenciesOfExternallyResolvedReferences",
            ]
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(&property.name))
            {
                bail!("MSBuild property '{}' is reserved by ms2cc", property.name);
            }
            insert_property(&mut properties, &property.name, &property.value)?;
        }
        Ok(properties)
    }

    fn insert_property(properties: &mut Properties, name: &str, value: &str) -> Result<()> {
        if let Some(existing) = properties
            .keys()
            .find(|existing| existing.eq_ignore_ascii_case(name))
        {
            bail!("MSBuild property '{name}' was specified more than once (also as '{existing}')");
        }
        properties.insert(name.to_owned(), value.to_owned());
        Ok(())
    }
}
