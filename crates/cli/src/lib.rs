use dsql_core::{
    Catalog, DefinitionRecord, Diagnostic, FragmentMap, QueryPlan, QueryRecord, SelectionPlan,
    SelectionPlanItem, Severity, SourceSnapshot, VariableBinding, check_query_definition,
    extract_definitions, generate_postgres_sql_with_options, infer_variable_bindings,
    lint_query_definition_with_options, parse_source, plan_query_definition,
};
use dsql_metadata::{
    BuildManifest, DynamicInputMetadata, HandoffMetadata, InputField, OperationMetadata,
    PolicyMetadata, ResultField, ResultShape, SourceMapEntry, SourceRange, SqlMetadata,
};
use miette::{IntoDiagnostic, Result};
use std::path::{Path, PathBuf};
use tokio::process::Command;

const BUILD_MANIFEST_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GenerateOptions {
    pub sql_collection_limit: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenerateOutput {
    pub manifest_path: PathBuf,
    pub generated_files: Vec<PathBuf>,
}

pub async fn generate_project_from(start_dir: &Path) -> Result<GenerateOutput> {
    generate_project_from_with_options(start_dir, GenerateOptions::default()).await
}

pub async fn generate_project_from_with_options(
    start_dir: &Path,
    options: GenerateOptions,
) -> Result<GenerateOutput> {
    let project = dsql_project::Project::load_from(start_dir)?;
    let base = project_root(&project);
    let catalog = project.load_catalog()?;
    let files = project_document_files(&project)?;
    if files.is_empty() {
        return Err(miette::miette!(
            "no dsql documents found in project {}",
            base.display()
        ));
    }

    let manifest = build_manifest(&project, &catalog, &files, options)?;
    let build_dir = project.root.join("build");
    tokio::fs::create_dir_all(&build_dir)
        .await
        .into_diagnostic()?;
    let manifest_path = build_dir.join("manifest.json");
    write_json(&manifest_path, &manifest).await?;

    let generated_files = vec![manifest_path.clone()];
    let typescript = &project.config.generate.typescript;
    if typescript.enabled {
        if typescript.cmd.is_empty() {
            return Err(miette::miette!(
                "generate.typescript.enabled requires generate.typescript.cmd"
            ));
        }
        let out_dir = resolve_project_path(&base, &typescript.out_dir);
        run_external_generator(&project, &manifest_path, &out_dir, &typescript.cmd).await?;
    }

    Ok(GenerateOutput {
        manifest_path,
        generated_files,
    })
}

pub async fn generate_typescript_metadata(out_dir: &Path) -> Result<()> {
    tokio::fs::create_dir_all(out_dir).await.map_err(|error| {
        miette::miette!(
            "failed to create generated output directory {}: {error}",
            out_dir.display()
        )
    })?;

    let schema = dsql_metadata::build_manifest_json_schema();
    let schema = serde_json::from_str::<serde_json::Value>(&schema)
        .and_then(|schema| serde_json::to_string_pretty(&schema))
        .into_diagnostic()?;
    tokio::fs::write(out_dir.join("build-manifest.schema.json"), schema)
        .await
        .map_err(|error| miette::miette!("failed to write build manifest schema: {error}"))?;

    tokio::fs::write(
        out_dir.join("metadata.ts"),
        dsql_metadata::build_manifest_typescript(),
    )
    .await
    .map_err(|error| miette::miette!("failed to write TypeScript metadata types: {error}"))?;
    Ok(())
}

fn build_manifest(
    project: &dsql_project::Project,
    catalog: &Catalog,
    files: &[PathBuf],
    options: GenerateOptions,
) -> Result<BuildManifest> {
    let mut fragments = FragmentMap::default();
    let mut queries = Vec::<LoadedQuery>::new();
    let mut parse_diagnostics = Vec::<Diagnostic>::new();

    for file in files {
        let text = std::fs::read_to_string(file)
            .map_err(|error| miette::miette!("failed to read {}: {error}", file.display()))?;
        let parsed = parse_source(SourceSnapshot::from_string(text));
        parse_diagnostics.extend(parsed.diagnostics.clone());
        let variables = infer_variable_bindings(&parsed.source_file, catalog);
        let extracted = extract_definitions(&parsed.source_file);
        for definition in extracted.definitions {
            match definition {
                DefinitionRecord::Query(query) => queries.push(LoadedQuery {
                    file: file.clone(),
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
            project, catalog, &fragments, &query, options,
        )?);
    }

    Ok(BuildManifest {
        version: BUILD_MANIFEST_VERSION,
        operations,
    })
}

fn build_query_operations(
    project: &dsql_project::Project,
    catalog: &Catalog,
    fragments: &FragmentMap,
    query: &LoadedQuery,
    options: GenerateOptions,
) -> Result<Vec<OperationMetadata>> {
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

        operations.push(OperationMetadata {
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
        });
    }
    Ok(operations)
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
    let file = query
        .file
        .strip_prefix(project_root(project))
        .unwrap_or(&query.file)
        .to_string_lossy()
        .to_string();
    vec![SourceMapEntry {
        id: query.query.key.name.clone().unwrap_or_default(),
        file,
        range: SourceRange {
            start: query.query.range.start,
            end: query.query.range.end,
        },
    }]
}

async fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let json = serde_json::to_string_pretty(value).into_diagnostic()?;
    tokio::fs::write(path, format!("{json}\n"))
        .await
        .map_err(|error| miette::miette!("failed to write {}: {error}", path.display()))?;
    Ok(())
}

async fn run_external_generator(
    project: &dsql_project::Project,
    manifest_path: &Path,
    out_dir: &Path,
    cmd: &[String],
) -> Result<()> {
    let Some(program) = cmd.first() else {
        return Ok(());
    };
    let base = project_root(project);
    let status = Command::new(program)
        .args(&cmd[1..])
        .current_dir(&base)
        .env("DSQL_PROJECT_DIR", &base)
        .env("DSQL_MANIFEST", manifest_path)
        .env("DSQL_OUT_DIR", out_dir)
        .status()
        .await
        .map_err(|error| miette::miette!("failed to run generator `{program}`: {error}"))?;
    if !status.success() {
        return Err(miette::miette!(
            "generator `{}` failed with status {}",
            cmd.join(" "),
            status
        ));
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

fn resolve_project_path(base: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

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
        let entry = entry.into_diagnostic()?;
        let path = entry.path();
        if path.is_dir() {
            collect_dsql_files(&path, excluded_dir, files)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("dsql") {
            files.push(path);
        }
    }
    Ok(())
}

struct LoadedQuery {
    file: PathBuf,
    query: QueryRecord,
    variables: Vec<VariableBinding>,
}
