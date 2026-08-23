use anyhow::{Context, Result};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use crate::{model::ProjectInstance, paths};

const ORIGINAL_IMPORTS_MARKER: &str = "<!-- MS2CC_ORIGINAL_IMPORTS -->";

const ADAPTER: &str = r#"<Project xmlns="http://schemas.microsoft.com/developer/msbuild/2003">
  <!-- MS2CC_ORIGINAL_IMPORTS -->
  <PropertyGroup>
    <_GlobalPropertiesToRemoveFromProjectReferences>$(_GlobalPropertiesToRemoveFromProjectReferences);Ms2ccScratchDirectory</_GlobalPropertiesToRemoveFromProjectReferences>
    <GetProjectInfoForReferenceDependsOn>$(GetProjectInfoForReferenceDependsOn);Ms2ccAddPublicBMIsToProjectsInfoForReference</GetProjectInfoForReferenceDependsOn>
  </PropertyGroup>
  <Target
    Name="Ms2ccAddPublicBMIsToProjectsInfoForReference"
    DependsOnTargets="$(BeforeClCompileTargets);$(ComputeCompileInputsTargets);GetPublicIncludeDirectories">
    <PropertyGroup>
      <_Ms2ccUseOnlySourcesInPublicDirectories
        Condition="'$(AllProjectBMIsArePublic)' != 'true'">true</_Ms2ccUseOnlySourcesInPublicDirectories>
    </PropertyGroup>
    <ItemGroup>
      <_Ms2ccPublicModules Include="@(ClCompile)"
        Condition="'%(ClCompile.CompileAs)' == 'CompileAsCppModule'" />
      <_Ms2ccPublicHeaderUnits Include="@(ClCompile)"
        Condition="'%(ClCompile.CompileAs)' == 'CompileAsHeaderUnit'" />
    </ItemGroup>
    <GetOutputFileNameItems
      Condition="'@(_Ms2ccPublicModules)' != ''"
      Sources="@(_Ms2ccPublicModules)"
      UseOnlySourcesInPublicDirectories="$(_Ms2ccUseOnlySourcesInPublicDirectories)"
      PublicDirectories="$(PublicModuleDirectories)"
      OutputMetadataName="ModuleOutputFile"
      OutputExtension="%(_Ms2ccPublicModules.Extension).ifc">
      <Output TaskParameter="OutputFiles" ItemName="_Ms2ccPublicModuleBmis" />
    </GetOutputFileNameItems>
    <GetOutputFileNameItems
      Condition="'@(_Ms2ccPublicHeaderUnits)' != ''"
      Sources="@(_Ms2ccPublicHeaderUnits)"
      UseOnlySourcesInPublicDirectories="$(_Ms2ccUseOnlySourcesInPublicDirectories)"
      PublicDirectories="$(PublicModuleDirectories);@(PublicIncludeDirectories)"
      OutputMetadataName="ModuleOutputFile"
      OutputExtension="%(_Ms2ccPublicHeaderUnits.Extension).ifc">
      <Output TaskParameter="OutputFiles" ItemName="_Ms2ccPublicHeaderUnitBmis" />
    </GetOutputFileNameItems>
    <ItemGroup>
      <ProjectInfoForReference Include="@(_Ms2ccPublicModuleBmis)">
        <FileType>Module</FileType>
      </ProjectInfoForReference>
      <ProjectInfoForReference Include="%(_Ms2ccPublicHeaderUnitBmis.FullPath)">
        <FileType>HeaderUnit</FileType>
      </ProjectInfoForReference>
      <_Ms2ccPublicModules Remove="@(_Ms2ccPublicModules)" />
      <_Ms2ccPublicHeaderUnits Remove="@(_Ms2ccPublicHeaderUnits)" />
      <_Ms2ccPublicModuleBmis Remove="@(_Ms2ccPublicModuleBmis)" />
      <_Ms2ccPublicHeaderUnitBmis Remove="@(_Ms2ccPublicHeaderUnitBmis)" />
    </ItemGroup>
  </Target>
  <UsingTask
    TaskName="SetModuleDependencies"
    AssemblyFile="$(CurrentVCTargetsPath)\Microsoft.Build.CPPTasks.Common.dll" />
  <Target Name="Ms2ccInjectReferencedBMIs">
    <ItemGroup>
      <ReferencedProjectsPublicHeaderUnits
        Include="$(Ms2ccReferencedHeaderUnitBMIs)"
        Condition="'$(Ms2ccReferencedHeaderUnitBMIs)' != ''" />
      <ReferencedProjectsPublicModules
        Include="$(Ms2ccReferencedModuleBMIs)"
        Condition="'$(Ms2ccReferencedModuleBMIs)' != ''" />
      <ClCompile Condition="'$(Ms2ccReferencedHeaderUnits)' != ''">
        <AdditionalHeaderUnitDependencies>$(Ms2ccReferencedHeaderUnits);%(ClCompile.AdditionalHeaderUnitDependencies)</AdditionalHeaderUnitDependencies>
      </ClCompile>
      <ClCompile Condition="'$(Ms2ccReferencedModules)' != ''">
        <AdditionalModuleDependencies>$(Ms2ccReferencedModules);%(ClCompile.AdditionalModuleDependencies)</AdditionalModuleDependencies>
      </ClCompile>
    </ItemGroup>
  </Target>
  <Target
    Name="Ms2ccResolveModuleDependencies"
    Condition="'$(EnableCppModules)' == 'true'">
    <Error
      Condition="'$(Ms2ccScratchDirectory)' == ''"
      Text="ms2cc did not provide a scratch directory for C++ module dependency scanning." />
    <MakeDir Directories="$(Ms2ccScratchDirectory)" />
    <ItemGroup>
      <ClCompile Condition="'%(ClCompile.ScanSourceForModuleDependencies)' == 'true'">
        <ModuleDependenciesFile>$(Ms2ccScratchDirectory)\</ModuleDependenciesFile>
      </ClCompile>
    </ItemGroup>
    <SetEnv Name="PATH" Value="$(ExecutablePath)" Prefix="false"
            Condition="'$(UseEnv)' != 'true'" />
    <SetEnv Name="LIB" Value="$(LibraryPath)" Prefix="false"
            Condition="'$(UseEnv)' != 'true'" />
    <SetEnv Name="LIBPATH" Value="$(ReferencePath)" Prefix="false"
            Condition="'$(UseEnv)' != 'true'" />
    <SetEnv Name="INCLUDE" Value="$(IncludePath)" Prefix="false"
            Condition="'$(UseEnv)' != 'true'" />
    <SetEnv Name="EXTERNAL_INCLUDE" Value="$(ExternalIncludePath)" Prefix="false"
            Condition="'$(UseEnv)' != 'true'" />
    <SetEnv Name="@(BuildMacro)" Value="%(BuildMacro.Value)" Prefix="false"
            Condition="'%(BuildMacro.EnvironmentVariable)' == 'true'" />
    <SetModuleDependencies
      Sources="@(ClCompile)"
      ReferencedHeaderUnitBMIs="@(ReferencedProjectsPublicHeaderUnits)"
      ReferencedModuleBMIs="@(ReferencedProjectsPublicModules)"
      TaskName="Microsoft.Build.CPPTasks.CL"
      TrackerLogDirectory="$(Ms2ccScratchDirectory)\"
      MinimalRebuildFromTracking="false"
      ToolArchitecture="$(CLToolArchitecture)"
      TrackerFrameworkPath="$(CLTrackerFrameworkPath)"
      TrackerSdkPath="$(CLTrackerSdkPath)"
      ToolExe="$(CLToolExe)"
      ToolPath="$(CLToolPath)"
      TrackFileAccess="false"
      UseStructuredOutput="$(UseClStructuredOutput)"
      YieldDuringToolExecution="$(CLYieldDuringToolExecution)"
      MaxProcessCount="$(MultiProcMaxCount)"
      EnforceProcessCountAcrossBuilds="$(EnforceProcessCountAcrossBuilds)"
      SchedulerName="$(MultiProcSchedulerName)"
      EnableExecuteTool="true">
      <Output TaskParameter="SourcesWithDependencies" ItemName="_Ms2ccSourcesWithDependencies" />
      <Output TaskParameter="AdditionalIncludePath" PropertyName="_Ms2ccReferencedHeaderUnitDirectories" />
    </SetModuleDependencies>
    <PropertyGroup Condition="'$(_Ms2ccReferencedHeaderUnitDirectories)' != ''">
      <INCLUDE>$(INCLUDE);$(_Ms2ccReferencedHeaderUnitDirectories)</INCLUDE>
      <IncludePath>$(IncludePath);$(_Ms2ccReferencedHeaderUnitDirectories)</IncludePath>
    </PropertyGroup>
    <ItemGroup>
      <ClCompile Remove="@(ClCompile)" />
      <ClCompile Include="@(_Ms2ccSourcesWithDependencies)" />
      <_Ms2ccSourcesWithDependencies Remove="@(_Ms2ccSourcesWithDependencies)" />
    </ItemGroup>
  </Target>
  <Target Name="Ms2ccCollectPublicBMIs"
          DependsOnTargets="GetPublicIncludeDirectories">
    <PropertyGroup>
      <_Ms2ccUseOnlyPublicSources
        Condition="'$(AllProjectBMIsArePublic)' != 'true'">true</_Ms2ccUseOnlyPublicSources>
    </PropertyGroup>
    <ItemGroup>
      <_Ms2ccOwnPublicModules Include="@(ClCompile)"
        Condition="'%(ClCompile.CompileAs)' == 'CompileAsCppModule'" />
      <_Ms2ccOwnPublicHeaderUnits Include="@(ClCompile)"
        Condition="'%(ClCompile.CompileAs)' == 'CompileAsHeaderUnit'" />
    </ItemGroup>
    <GetOutputFileNameItems
      Condition="'@(_Ms2ccOwnPublicModules)' != ''"
      Sources="@(_Ms2ccOwnPublicModules)"
      UseOnlySourcesInPublicDirectories="$(_Ms2ccUseOnlyPublicSources)"
      PublicDirectories="$(PublicModuleDirectories)"
      OutputMetadataName="ModuleOutputFile"
      OutputExtension="%(_Ms2ccOwnPublicModules.Extension).ifc">
      <Output TaskParameter="OutputFiles" ItemName="_Ms2ccOwnPublicModuleBmis" />
    </GetOutputFileNameItems>
    <GetOutputFileNameItems
      Condition="'@(_Ms2ccOwnPublicHeaderUnits)' != ''"
      Sources="@(_Ms2ccOwnPublicHeaderUnits)"
      UseOnlySourcesInPublicDirectories="$(_Ms2ccUseOnlyPublicSources)"
      PublicDirectories="$(PublicModuleDirectories);@(PublicIncludeDirectories)"
      OutputMetadataName="ModuleOutputFile"
      OutputExtension="%(_Ms2ccOwnPublicHeaderUnits.Extension).ifc">
      <Output TaskParameter="OutputFiles" ItemName="_Ms2ccOwnPublicHeaderUnitBmis" />
    </GetOutputFileNameItems>
  </Target>
  <Target
    Name="Ms2ccGetCompileCommands"
    DependsOnTargets="$(BeforeClCompileTargets);$(DesignTimeBuildInitTargets);$(ComputeCompileInputsTargets);Ms2ccInjectReferencedBMIs;Ms2ccResolveModuleDependencies;Ms2ccCollectPublicBMIs;GetProjectDirectories;GetClCommandLines"
    Returns="@(_Ms2ccResult)">
    <PropertyGroup>
      <_Ms2ccHasProjectReferences Condition="'@(ProjectReference)' != ''">true</_Ms2ccHasProjectReferences>
    </PropertyGroup>
    <ItemGroup>
      <_Ms2ccResult Include="ms2cc-command">
        <Kind>Command</Kind>
        <CommandLine>%(ClCommandLines.Identity)</CommandLine>
        <WorkingDirectory>%(ClCommandLines.WorkingDirectory)</WorkingDirectory>
        <ToolPath>%(ClCommandLines.ToolPath)</ToolPath>
        <Files>%(ClCommandLines.Files)</Files>
        <ConfigurationOptions>%(ClCommandLines.ConfigurationOptions)</ConfigurationOptions>
        <ContainsReferencedProjectsData>%(ClCommandLines.ContainsReferencedProjectsData)</ContainsReferencedProjectsData>
        <ProjectFile>$(MSBuildProjectFullPath)</ProjectFile>
        <Configuration>$(Configuration)</Configuration>
        <Platform>$(Platform)</Platform>
        <CLToolExe>$(CLToolExe)</CLToolExe>
        <CLToolPath>$(CLToolPath)</CLToolPath>
        <ExecutablePath>$(ExecutablePath)</ExecutablePath>
      </_Ms2ccResult>
      <_Ms2ccResult Include="ms2cc-directories">
        <Kind>Directories</Kind>
        <IncludePath>%(_ProjectDirectories.IncludePath)</IncludePath>
        <ExternalIncludePath>%(_ProjectDirectories.ExternalIncludePath)</ExternalIncludePath>
        <FrameworkIncludePath>%(_ProjectDirectories.FrameworkIncludePath)</FrameworkIncludePath>
        <EnvironmentCL>$(CL)</EnvironmentCL>
        <EnvironmentTrailingCL>$(_CL_)</EnvironmentTrailingCL>
        <EnvironmentExternalInclude>$(EXTERNAL_INCLUDE)</EnvironmentExternalInclude>
        <UseEnv>$(UseEnv)</UseEnv>
        <HasProjectReferences>$(_Ms2ccHasProjectReferences)</HasProjectReferences>
        <ReferenceResolutionDisabled>$(ReferenceResolutionDisabled)</ReferenceResolutionDisabled>
        <ProjectFile>$(MSBuildProjectFullPath)</ProjectFile>
      </_Ms2ccResult>
      <_Ms2ccResult Include="ms2cc-source">
        <Kind>Source</Kind>
        <Source>%(ClCompile.FullPath)</Source>
        <ExcludedFromBuild>%(ClCompile.ExcludedFromBuild)</ExcludedFromBuild>
        <ConfigurationOptions>%(ClCompile.ConfigurationOptions)</ConfigurationOptions>
        <ForcedUsingFiles>%(ClCompile.ForcedUsingFiles)</ForcedUsingFiles>
        <CompileAs>%(ClCompile.CompileAs)</CompileAs>
        <CompilerIteration>%(ClCompile.CompilerIteration)</CompilerIteration>
        <ModuleOutputFile>%(ClCompile.ModuleOutputFile)</ModuleOutputFile>
        <ObjectFileName>%(ClCompile.ObjectFileName)</ObjectFileName>
        <GenerateSourceDependencies>%(ClCompile.GenerateSourceDependencies)</GenerateSourceDependencies>
        <SourceDependenciesFile>%(ClCompile.SourceDependenciesFile)</SourceDependenciesFile>
        <ProjectFile>$(MSBuildProjectFullPath)</ProjectFile>
      </_Ms2ccResult>
      <_Ms2ccResult Include="ms2cc-public-header-unit">
        <Kind>PublicHeaderUnit</Kind>
        <Bmi>%(_Ms2ccOwnPublicHeaderUnitBmis.FullPath)</Bmi>
        <ProjectFile>$(MSBuildProjectFullPath)</ProjectFile>
      </_Ms2ccResult>
      <_Ms2ccResult Include="ms2cc-public-module">
        <Kind>PublicModule</Kind>
        <Bmi>%(_Ms2ccOwnPublicModuleBmis.FullPath)</Bmi>
        <ProjectFile>$(MSBuildProjectFullPath)</ProjectFile>
      </_Ms2ccResult>
    </ItemGroup>
  </Target>
</Project>
"#;

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
