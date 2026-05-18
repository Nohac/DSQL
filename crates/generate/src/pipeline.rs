use dsql_core::{
    Catalog, DefinitionRecord, Diagnostic, FragmentMap, QueryPlan, QueryRecord, SelectionPlan,
    SelectionPlanItem, Severity, SourceSnapshot, VariableBinding, check_query_definition,
    extract_definitions, generate_postgres_sql_with_options, infer_variable_bindings,
    lint_query_definition_with_options, parse_source, plan_query_definition,
};
use dsql_metadata::{
    BuildManifest, DynamicInputMetadata, HandoffMetadata, InputField, OperationManifestEntry,
    OperationMetadata, PolicyMetadata, ResultField, ResultShape, SourceMapEntry, SourceRange,
    SqlMetadata,
};
use miette::Result;
use std::path::{Path, PathBuf};

use crate::{
    artifacts::{ArtifactWriter, OperationArtifact, WrittenArtifacts, WrittenOperationArtifact},
    runner::{GenerateTarget, GeneratorRunner},
};

const BUILD_MANIFEST_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GenerateOptions {
    pub sql_collection_limit: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct GenerateDocument {
    pub path: PathBuf,
    pub text: String,
}

#[derive(Clone, Debug)]
pub(crate) struct GenerateInput {
    pub project: dsql_project::Project,
    pub catalog: Catalog,
    pub documents: Vec<GenerateDocument>,
    pub options: GenerateOptions,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerateOutput {
    pub manifest_path: String,
    pub operation_paths: Vec<String>,
}

pub(crate) async fn generate_project<W, R>(
    input: GenerateInput,
    writer: &W,
    runner: &R,
) -> Result<GenerateOutput>
where
    W: ArtifactWriter,
    R: GeneratorRunner,
{
    if input.documents.is_empty() {
        return Err(miette::miette!(
            "no dsql documents found in project {}",
            project_root(&input.project).display()
        ));
    }

    let operations = build_operations(&input)?;
    let mut written_operations = Vec::with_capacity(operations.len());
    for operation in operations {
        let reference = writer.write_operation(&operation).await?;
        written_operations.push(WrittenOperationArtifact {
            metadata: operation.metadata,
            reference,
            hash: operation.hash,
            source: operation.source,
        });
    }

    let manifest = manifest_from_written_operations(&written_operations);
    let manifest_ref = writer.write_manifest(&manifest).await?;
    let artifacts = WrittenArtifacts {
        manifest: manifest_ref,
        operations: written_operations,
    };

    let typescript = &input.project.config.generate.typescript;
    if typescript.enabled {
        if typescript.cmd.is_empty() {
            return Err(miette::miette!(
                "generate.typescript.enabled requires generate.typescript.cmd"
            ));
        }
        let base = project_root(&input.project);
        let target = GenerateTarget {
            project_dir: base.to_string_lossy().to_string(),
            out_dir: resolve_project_path(&base, &typescript.out_dir)
                .to_string_lossy()
                .to_string(),
            cmd: typescript.cmd.clone(),
        };
        runner.run(&target, &artifacts).await?;
    }

    Ok(GenerateOutput {
        manifest_path: artifacts.manifest.path,
        operation_paths: artifacts
            .operations
            .iter()
            .map(|operation| operation.reference.path.clone())
            .collect(),
    })
}

#[cfg(all(feature = "fs", feature = "process"))]
pub async fn generate_project_from(start_dir: &Path) -> Result<GenerateOutput> {
    generate_project_from_with_options(start_dir, GenerateOptions::default()).await
}

#[cfg(all(feature = "fs", feature = "process"))]
pub async fn generate_project_from_with_options(
    start_dir: &Path,
    options: GenerateOptions,
) -> Result<GenerateOutput> {
    let project = dsql_project::Project::load_from(start_dir)?;
    let catalog = project.load_catalog()?;
    let documents = load_project_documents(&project)?;
    let writer = crate::fs::FsArtifactWriter::new(project.root.join("build"));
    let runner = crate::process::CommandGeneratorRunner;
    generate_project(
        GenerateInput {
            project,
            catalog,
            documents,
            options,
        },
        &writer,
        &runner,
    )
    .await
}

fn build_operations(input: &GenerateInput) -> Result<Vec<OperationArtifact>> {
    let mut fragments = FragmentMap::default();
    let mut queries = Vec::<LoadedQuery>::new();
    let mut parse_diagnostics = Vec::<Diagnostic>::new();

    for document in &input.documents {
        let parsed = parse_source(SourceSnapshot::from_string(document.text.clone()));
        parse_diagnostics.extend(parsed.diagnostics.clone());
        let variables = infer_variable_bindings(&parsed.source_file, &input.catalog);
        let extracted = extract_definitions(&parsed.source_file);
        for definition in extracted.definitions {
            match definition {
                DefinitionRecord::Query(query) => queries.push(LoadedQuery {
                    file: document.path.clone(),
                    variables: variables
                        .bindings
                        .iter()
                        .filter(|binding| {
                            binding.range.start >= query.range.start
                                && binding.range.end <= query.range.end
                        })
                        .cloned()
                        .collect(),
                    query,
                }),
                DefinitionRecord::Fragment(fragment) => fragments.insert(fragment),
            }
        }
    }

    fail_on_error_diagnostics(parse_diagnostics)?;

    let mut operations = Vec::new();
    for query in queries {
        operations.extend(build_query_operations(
            &input.project,
            &input.catalog,
            &fragments,
            &query,
            input.options,
        )?);
    }
    Ok(operations)
}

fn build_query_operations(
    project: &dsql_project::Project,
    catalog: &Catalog,
    fragments: &FragmentMap,
    query: &LoadedQuery,
    options: GenerateOptions,
) -> Result<Vec<OperationArtifact>> {
    let checked = check_query_definition(&query.query, fragments, catalog);
    let linted = lint_query_definition_with_options(
        &query.query,
        fragments,
        catalog,
        project.lint_options(),
    );
    let planned = plan_query_definition(&query.query, fragments, catalog);

    let mut diagnostics = Vec::new();
    diagnostics.extend(checked.diagnostics);
    diagnostics.extend(linted.diagnostics);
    diagnostics.extend(planned.diagnostics);
    fail_on_error_diagnostics(diagnostics)?;

    let query_name = query
        .query
        .key
        .name
        .as_deref()
        .ok_or_else(|| miette::miette!("anonymous queries cannot be generated"))?;
    let mut operations = Vec::new();
    for (index, plan) in planned.queries.iter().enumerate() {
        let generated = generate_postgres_sql_with_options(
            plan,
            catalog,
            dsql_core::PostgresSqlOptions {
                collection_limit: options.sql_collection_limit,
            },
        )
        .map_err(|error| miette::miette!("failed to generate SQL for `{query_name}`: {error}"))?;

        let metadata = OperationMetadata {
            name: operation_name(
                query_name,
                &generated.output_name,
                planned.queries.len(),
                index,
            ),
            kind: "query".to_string(),
            sql: SqlMetadata {
                dialect: "postgres".to_string(),
                text: generated.sql,
                variants: Vec::new(),
            },
            result: result_shape(catalog, plan)?,
            params: input_fields(&query.variables, true),
            input: input_fields(&query.variables, false),
            context: Vec::new(),
            dynamic_inputs: dynamic_inputs(&query.variables),
            policies: Vec::<PolicyMetadata>::new(),
            handoffs: Vec::<HandoffMetadata>::new(),
            source_map: source_map(project, query),
        };
        let hash = stable_hash(&serde_json::to_string(&metadata).map_err(|error| {
            miette::miette!("failed to hash operation `{}`: {error}", metadata.name)
        })?);
        operations.push(OperationArtifact {
            metadata,
            hash,
            source: source_path(project, &query.file),
        });
    }
    Ok(operations)
}

fn manifest_from_written_operations(operations: &[WrittenOperationArtifact]) -> BuildManifest {
    BuildManifest {
        version: BUILD_MANIFEST_VERSION,
        operations: operations
            .iter()
            .map(|operation| OperationManifestEntry {
                name: operation.metadata.name.clone(),
                kind: operation.metadata.kind.clone(),
                path: operation.reference.path.clone(),
                hash: operation.hash.clone(),
                source: operation.source.clone(),
            })
            .collect(),
    }
}

fn fail_on_error_diagnostics(mut diagnostics: Vec<Diagnostic>) -> Result<()> {
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    let errors = diagnostics
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    if errors.is_empty() {
        return Ok(());
    }

    let mut message = String::from("cannot generate while diagnostics contain errors");
    for diagnostic in errors {
        message.push_str(&format!(
            "\n{:?} {:?} {}..{}: {}",
            diagnostic.source,
            diagnostic.code,
            diagnostic.range.start,
            diagnostic.range.end,
            diagnostic.message
        ));
    }
    Err(miette::miette!("{message}"))
}

fn result_shape(catalog: &Catalog, plan: &QueryPlan) -> Result<ResultShape> {
    let mut fields = Vec::new();
    collect_result_fields(
        catalog,
        "",
        &plan.output_name,
        &plan.selections,
        "array",
        &mut fields,
    )?;
    Ok(ResultShape { fields })
}

fn collect_result_fields(
    catalog: &Catalog,
    parent_path: &str,
    name: &str,
    selection: &SelectionPlan,
    kind: &str,
    fields: &mut Vec<ResultField>,
) -> Result<()> {
    let path = join_path(parent_path, name);
    fields.push(ResultField {
        path: path.clone(),
        name: name.to_string(),
        parent_path: parent_path.to_string(),
        kind: kind.to_string(),
        data_type: "object".to_string(),
        nullable: false,
    });

    for item in &selection.items {
        match item {
            SelectionPlanItem::Projection(projection) => {
                let column = catalog
                    .column_by_id(projection.column)
                    .ok_or_else(|| miette::miette!("missing projected column"))?;
                fields.push(ResultField {
                    path: join_path(&path, &projection.output_name),
                    name: projection.output_name.clone(),
                    parent_path: path.clone(),
                    kind: "scalar".to_string(),
                    data_type: column.data_type.as_str().to_string(),
                    nullable: !column.not_null,
                });
            }
            SelectionPlanItem::Relation(relation) => {
                collect_result_fields(
                    catalog,
                    &path,
                    &relation.output_name,
                    &relation.selections,
                    "object",
                    fields,
                )?;
            }
        }
    }
    Ok(())
}

fn input_fields(variables: &[VariableBinding], top_level: bool) -> Vec<InputField> {
    variables
        .iter()
        .filter(|binding| {
            matches!(
                (top_level, binding.source),
                (true, dsql_core::VariableSource::TopLevel)
                    | (false, dsql_core::VariableSource::Structured)
            )
        })
        .map(|binding| InputField {
            path: binding.path.clone(),
            data_type: binding.data_type.as_str().to_string(),
            required: true,
            nullable: false,
        })
        .collect()
}

fn dynamic_inputs(variables: &[VariableBinding]) -> Vec<DynamicInputMetadata> {
    variables
        .iter()
        .filter(|binding| !binding.enum_values.is_empty())
        .map(|binding| DynamicInputMetadata {
            name: binding.name.clone().unwrap_or_else(|| {
                binding
                    .path
                    .rsplit('.')
                    .next()
                    .unwrap_or("value")
                    .to_string()
            }),
            kind: format!("{:?}", binding.role).to_ascii_lowercase(),
            preset: String::new(),
            fields: Vec::new(),
        })
        .collect()
}

fn source_map(project: &dsql_project::Project, query: &LoadedQuery) -> Vec<SourceMapEntry> {
    vec![SourceMapEntry {
        id: query.query.key.name.clone().unwrap_or_default(),
        file: source_path(project, &query.file),
        range: SourceRange {
            start: query.query.range.start,
            end: query.query.range.end,
        },
    }]
}

#[cfg(all(feature = "fs", feature = "process"))]
fn load_project_documents(project: &dsql_project::Project) -> Result<Vec<GenerateDocument>> {
    project_document_files(project)?
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path)
                .map_err(|error| miette::miette!("failed to read {}: {error}", path.display()))?;
            Ok(GenerateDocument { path, text })
        })
        .collect()
}

#[cfg(all(feature = "fs", feature = "process"))]
fn project_document_files(project: &dsql_project::Project) -> Result<Vec<PathBuf>> {
    let base = project_root(project);
    let mut files = Vec::new();
    if project.config.documents.is_empty() {
        collect_dsql_files(&base, Some(&project.root), &mut files)?;
    } else {
        for document in &project.config.documents {
            if !matches!(document.resolver, dsql_project::ResolverType::Dsql) {
                continue;
            }
            for path in &document.paths {
                let path = base.join(path);
                collect_document_path(&path, Some(&project.root), &mut files)?;
            }
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn project_root(project: &dsql_project::Project) -> PathBuf {
    project
        .root
        .parent()
        .map_or_else(|| project.root.clone(), Path::to_path_buf)
}

fn source_path(project: &dsql_project::Project, file: &Path) -> String {
    file.strip_prefix(project_root(project))
        .unwrap_or(file)
        .to_string_lossy()
        .to_string()
}

fn resolve_project_path(base: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[cfg(all(feature = "fs", feature = "process"))]
fn collect_document_path(
    path: &Path,
    excluded_dir: Option<&Path>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if path.is_dir() {
        collect_dsql_files(path, excluded_dir, files)
    } else if path.extension().and_then(|ext| ext.to_str()) == Some("dsql") {
        files.push(path.to_path_buf());
        Ok(())
    } else {
        Err(miette::miette!(
            "dsql document path not found: {}",
            path.display()
        ))
    }
}

#[cfg(all(feature = "fs", feature = "process"))]
fn collect_dsql_files(
    dir: &Path,
    excluded_dir: Option<&Path>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if excluded_dir.is_some_and(|excluded| dir == excluded) {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)
        .map_err(|error| miette::miette!("failed to read directory {}: {error}", dir.display()))?
    {
        let entry =
            entry.map_err(|error| miette::miette!("failed to read directory entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_dsql_files(&path, excluded_dir, files)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("dsql") {
            files.push(path);
        }
    }
    Ok(())
}

fn operation_name(query_name: &str, output_name: &str, count: usize, index: usize) -> String {
    if count == 1 {
        query_name.to_string()
    } else {
        format!("{query_name}_{output_name}_{index}")
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}.{name}")
    }
}

fn stable_hash(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

struct LoadedQuery {
    file: PathBuf,
    query: QueryRecord,
    variables: Vec<VariableBinding>,
}
