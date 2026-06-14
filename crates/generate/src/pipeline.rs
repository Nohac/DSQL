use dsql_core::{
    Catalog, CompilerDiagnostic, DefinitionRecord, DefinitionResolver, Diagnostic, DsqlDiagnostic,
    FieldCheckResult, FragmentMap, FragmentRecord, InputPathSegment, QueryPlan, QueryRecord,
    Selection, SelectionClauses, SelectionKind, SelectionPlan, SelectionPlanItem, Severity,
    SourceSnapshot, SqlValue, TableId, VariableBinding, check_fragment_definition,
    check_query_definition, collect_checked_compiler_diagnostics,
    collect_query_compiler_diagnostics, duplicate_fragment_errors, extract_definitions,
    generate_postgres_sql_with_options, infer_fragment_variable_bindings,
    infer_query_variable_bindings, is_input_path, is_params_path,
    lint_query_definition_with_options, parse_source, plan_fragment_definition,
    plan_query_definition,
};
use dsql_metadata::{
    BuildManifest, DefinitionKind, DynamicInputMetadata, FragmentManifestEntry, FragmentMetadata,
    FragmentSpreadMetadata, HandoffMetadata, InputField, OperationManifestEntry, OperationMetadata,
    PolicyMetadata, ResultDataType, ResultField, ResultFieldKind, ResultShape, SourceMapEntry,
    SourceRange, SqlDialect, SqlMetadata, SqlParameterMetadata, SqlVariantCaseMetadata,
    SqlVariantMetadata,
};
use facet::Facet;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
};

use crate::{
    GenerateError, Result,
    artifacts::{
        ArtifactWriter, FragmentArtifact, OperationArtifact, WrittenArtifacts,
        WrittenFragmentArtifact, WrittenOperationArtifact,
    },
    layout::{BUILD_DIR, MANIFEST_FILE, fragment_manifest_path, operation_manifest_path},
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
    pub source_offset: u32,
    pub resolution_scope: String,
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

#[derive(Clone, Debug)]
pub struct ValidationOutput {
    pub document_count: usize,
    pub query_count: usize,
    pub diagnostics: Vec<ValidationDiagnostic>,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ValidationDiagnostic {
    pub file: PathBuf,
    pub source_offset: u32,
    pub diagnostic: Diagnostic,
}

#[derive(Clone, Debug, Facet)]
pub struct GeneratedOperationArtifact {
    pub metadata: OperationMetadata,
    pub hash: String,
    pub source: String,
}

#[derive(Clone, Debug, Facet)]
pub struct GeneratedFragmentArtifact {
    pub metadata: FragmentMetadata,
    pub hash: String,
    pub source: String,
}

#[derive(Clone, Debug, Facet)]
pub struct GeneratedResolutionScope {
    pub name: String,
    pub imports: Vec<String>,
}

#[derive(Clone, Debug, Facet)]
pub struct GeneratedSourceScope {
    pub file: String,
    pub source_offset: u32,
    pub scope: String,
}

#[derive(Clone, Debug, Facet)]
pub struct GeneratedArtifactGroup {
    pub name: String,
    pub imports: Vec<String>,
    pub manifest: BuildManifest,
    pub operations: Vec<GeneratedOperationArtifact>,
    pub fragments: Vec<GeneratedFragmentArtifact>,
    pub source_file_scopes: Vec<GeneratedSourceScope>,
}

#[derive(Clone, Debug, Facet)]
pub struct GeneratedArtifacts {
    pub project_dir: String,
    pub manifest_path: String,
    pub scopes: Vec<GeneratedResolutionScope>,
    pub source_file_scopes: Vec<GeneratedSourceScope>,
    pub manifest: BuildManifest,
    pub operations: Vec<GeneratedOperationArtifact>,
    pub fragments: Vec<GeneratedFragmentArtifact>,
    pub artifact_groups: Vec<GeneratedArtifactGroup>,
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
        return Err(GenerateError::NoDocuments {
            project: project_root(&input.project).display().to_string(),
        });
    }

    let built = build_artifacts(&input)?;
    let operations = built.operations;
    let fragments = built.fragments;
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
    let mut written_fragments = Vec::with_capacity(fragments.len());
    for fragment in fragments {
        let reference = writer.write_fragment(&fragment).await?;
        written_fragments.push(WrittenFragmentArtifact {
            metadata: fragment.metadata,
            reference,
            hash: fragment.hash,
            source: fragment.source,
        });
    }

    let manifest = manifest_from_written_artifacts(&written_operations, &written_fragments);
    let manifest_ref = writer.write_manifest(&manifest).await?;
    let artifacts = WrittenArtifacts {
        manifest: manifest_ref,
        operations: written_operations,
        fragments: written_fragments,
    };

    let typescript = &input.project.config.generate.typescript;
    if typescript.enabled {
        if typescript.cmd.is_empty() {
            return Err(miette::miette!(
                "generate.typescript.enabled requires generate.typescript.cmd"
            )
            .into());
        }
        let base = project_root(&input.project);
        let target = GenerateTarget {
            project_dir: base.to_string_lossy().to_string(),
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

pub(crate) fn generate_project_artifacts(input: GenerateInput) -> Result<GeneratedArtifacts> {
    if input.documents.is_empty() {
        return Err(GenerateError::NoDocuments {
            project: project_root(&input.project).display().to_string(),
        });
    }

    let built = build_artifacts(&input)?;
    let base = project_root(&input.project);
    let scopes = generated_resolution_scopes(&input.project);
    let source_file_scopes = generated_source_scopes(&input.documents);
    let mut generated_operations = Vec::new();
    let mut generated_fragments = Vec::new();
    let mut operation_manifest_entries = Vec::new();
    let mut fragment_manifest_entries = Vec::new();
    let mut artifact_groups = Vec::new();

    for group in built.groups {
        let mut group_operations = Vec::new();
        let mut group_fragments = Vec::new();
        let mut group_operation_manifest_entries = Vec::new();
        let mut group_fragment_manifest_entries = Vec::new();

        for operation in group.operations {
            let operation_path = operation_manifest_path(&operation.metadata.name);
            let entry = OperationManifestEntry {
                name: operation.metadata.name.clone(),
                kind: operation.metadata.kind.clone(),
                path: operation_path,
                hash: operation.hash.clone(),
                source: operation.source.clone(),
            };
            let generated = GeneratedOperationArtifact {
                metadata: operation.metadata,
                hash: operation.hash,
                source: operation.source,
            };
            operation_manifest_entries.push(entry.clone());
            group_operation_manifest_entries.push(entry);
            generated_operations.push(generated.clone());
            group_operations.push(generated);
        }

        for fragment in group.fragments {
            let fragment_path = fragment_manifest_path(&fragment.metadata.name);
            let entry = FragmentManifestEntry {
                name: fragment.metadata.name.clone(),
                kind: fragment.metadata.kind.clone(),
                path: fragment_path,
                hash: fragment.hash.clone(),
                source: fragment.source.clone(),
            };
            let generated = GeneratedFragmentArtifact {
                metadata: fragment.metadata,
                hash: fragment.hash,
                source: fragment.source,
            };
            fragment_manifest_entries.push(entry.clone());
            group_fragment_manifest_entries.push(entry);
            generated_fragments.push(generated.clone());
            group_fragments.push(generated);
        }

        artifact_groups.push(GeneratedArtifactGroup {
            name: group.name,
            imports: group.imports,
            manifest: BuildManifest {
                version: BUILD_MANIFEST_VERSION,
                operations: group_operation_manifest_entries,
                fragments: group_fragment_manifest_entries,
            },
            operations: group_operations,
            fragments: group_fragments,
            source_file_scopes: source_file_scopes
                .iter()
                .filter(|source| source.scope == group.scope_name)
                .cloned()
                .collect(),
        });
    }

    Ok(GeneratedArtifacts {
        project_dir: base.to_string_lossy().to_string(),
        manifest_path: input
            .project
            .root
            .join(BUILD_DIR)
            .join(MANIFEST_FILE)
            .to_string_lossy()
            .to_string(),
        scopes,
        source_file_scopes,
        manifest: BuildManifest {
            version: BUILD_MANIFEST_VERSION,
            operations: operation_manifest_entries,
            fragments: fragment_manifest_entries,
        },
        operations: generated_operations,
        fragments: generated_fragments,
        artifact_groups,
    })
}

fn generated_resolution_scopes(project: &dsql_project::Project) -> Vec<GeneratedResolutionScope> {
    if project.config.resolution.is_empty() {
        return vec![GeneratedResolutionScope {
            name: dsql_project::DEFAULT_RESOLUTION_SCOPE.to_string(),
            imports: Vec::new(),
        }];
    }

    project
        .config
        .resolution
        .iter()
        .map(|(name, config)| GeneratedResolutionScope {
            name: name.clone(),
            imports: config.imports.clone(),
        })
        .collect()
}

fn generated_source_scopes(documents: &[GenerateDocument]) -> Vec<GeneratedSourceScope> {
    documents
        .iter()
        .map(|document| GeneratedSourceScope {
            file: document.path.to_string_lossy().to_string(),
            source_offset: document.source_offset,
            scope: document.resolution_scope.clone(),
        })
        .collect()
}

pub(crate) fn validate_project(input: GenerateInput) -> ValidationOutput {
    let mut queries = Vec::<LoadedQuery>::new();
    let mut loaded_fragments = Vec::<LoadedFragment>::new();
    let mut diagnostics = Vec::<ValidationDiagnostic>::new();
    let mut errors = Vec::<String>::new();

    for document in &input.documents {
        let parsed = parse_source(SourceSnapshot::from_string(document.text.clone()));
        diagnostics.extend(
            parsed
                .diagnostics
                .iter()
                .cloned()
                .map(|diagnostic| validation_diagnostic(document, diagnostic)),
        );
        let extracted = extract_definitions(&parsed.source_file);
        for definition in extracted.definitions {
            match definition {
                DefinitionRecord::Query(query) => queries.push(LoadedQuery {
                    file: document.path.clone(),
                    source_offset: document.source_offset,
                    resolution_scope: document.resolution_scope.clone(),
                    query,
                }),
                DefinitionRecord::Fragment(fragment) => {
                    loaded_fragments.push(LoadedFragment {
                        file: document.path.clone(),
                        source_offset: document.source_offset,
                        resolution_scope: document.resolution_scope.clone(),
                        fragment,
                    });
                }
            }
        }
    }

    let query_count = queries.len();
    match scope_definitions(&input, queries, loaded_fragments) {
        Ok(scopes) => {
            for scope in scopes {
                let fragments = scope.fragment_map();
                for fragment in &scope.fragments {
                    let checked =
                        check_fragment_definition(&fragment.fragment, &fragments, &input.catalog);
                    diagnostics.extend(
                        checked
                            .diagnostics
                            .into_iter()
                            .map(|diagnostic| fragment_validation_diagnostic(fragment, diagnostic)),
                    );
                }
                diagnostics.extend(
                    duplicate_fragment_errors(&fragments)
                        .into_iter()
                        .filter_map(|diagnostic| {
                            scope
                                .fragments
                                .iter()
                                .find(|fragment| {
                                    fragment.fragment.key.name.as_str()
                                        == match &diagnostic.kind {
                                            dsql_core::CheckDiagnosticKind::DuplicateFragment {
                                                name,
                                            } => name.as_str(),
                                            _ => return false,
                                        }
                                        && fragment.fragment.name_range == diagnostic.range
                                })
                                .map(|fragment| {
                                    fragment_validation_diagnostic(fragment, diagnostic)
                                })
                        }),
                );

                for query in &scope.queries {
                    let checked = check_query_definition(&query.query, &fragments, &input.catalog);
                    let linted = lint_query_definition_with_options(
                        &query.query,
                        &fragments,
                        &input.catalog,
                        input.project.lint_options(),
                    );
                    let planned = plan_query_definition(&query.query, &fragments, &input.catalog);
                    diagnostics.extend(
                        checked
                            .diagnostics
                            .into_iter()
                            .map(|diagnostic| query_validation_diagnostic(query, diagnostic))
                            .chain(
                                linted.diagnostics.into_iter().map(|diagnostic| {
                                    query_validation_diagnostic(query, diagnostic)
                                }),
                            )
                            .chain(
                                planned.diagnostics.into_iter().map(|diagnostic| {
                                    query_validation_diagnostic(query, diagnostic)
                                }),
                            ),
                    );
                    if let Some(query_name) = query.query.key.name.as_deref() {
                        let variables =
                            infer_query_variable_bindings(&query.query, &fragments, &input.catalog)
                                .bindings;
                        if let Err(error) = validate_variable_bindings(query_name, &variables) {
                            errors.push(error.to_string());
                        }
                    } else {
                        errors.push("anonymous queries cannot be generated".to_string());
                    }
                }
            }
        }
        Err(error) => errors.push(error.to_string()),
    }

    diagnostics.sort_by(|left, right| {
        left.file.cmp(&right.file).then(
            (left.source_offset + left.diagnostic.range.start)
                .cmp(&(right.source_offset + right.diagnostic.range.start)),
        )
    });
    errors.sort();
    ValidationOutput {
        document_count: input.documents.len(),
        query_count,
        diagnostics,
        errors,
    }
}

fn validation_diagnostic(
    document: &GenerateDocument,
    diagnostic: Diagnostic,
) -> ValidationDiagnostic {
    ValidationDiagnostic {
        file: document.path.clone(),
        source_offset: document.source_offset,
        diagnostic,
    }
}

fn query_validation_diagnostic(
    query: &LoadedQuery,
    diagnostic: impl DsqlDiagnostic,
) -> ValidationDiagnostic {
    ValidationDiagnostic {
        file: query.file.clone(),
        source_offset: query.source_offset,
        diagnostic: diagnostic.to_transport(),
    }
}

fn fragment_validation_diagnostic(
    fragment: &LoadedFragment,
    diagnostic: impl DsqlDiagnostic,
) -> ValidationDiagnostic {
    ValidationDiagnostic {
        file: fragment.file.clone(),
        source_offset: fragment.source_offset,
        diagnostic: diagnostic.to_transport(),
    }
}

#[derive(Clone, Debug)]
struct BuiltArtifacts {
    operations: Vec<OperationArtifact>,
    fragments: Vec<FragmentArtifact>,
    groups: Vec<BuiltArtifactGroup>,
}

#[derive(Clone, Debug)]
struct BuiltArtifactGroup {
    name: String,
    scope_name: String,
    imports: Vec<String>,
    operations: Vec<OperationArtifact>,
    fragments: Vec<FragmentArtifact>,
}

fn build_artifacts(input: &GenerateInput) -> Result<BuiltArtifacts> {
    let mut queries = Vec::<LoadedQuery>::new();
    let mut loaded_fragments = Vec::<LoadedFragment>::new();
    let mut parse_diagnostics = Vec::<CompilerDiagnostic>::new();

    for document in &input.documents {
        let parsed = parse_source(SourceSnapshot::from_string(document.text.clone()));
        parse_diagnostics.extend(parsed.diagnostics.clone().into_iter().map(Into::into));
        let extracted = extract_definitions(&parsed.source_file);
        for definition in extracted.definitions {
            match definition {
                DefinitionRecord::Query(query) => queries.push(LoadedQuery {
                    file: document.path.clone(),
                    source_offset: document.source_offset,
                    resolution_scope: document.resolution_scope.clone(),
                    query,
                }),
                DefinitionRecord::Fragment(fragment) => {
                    loaded_fragments.push(LoadedFragment {
                        file: document.path.clone(),
                        source_offset: document.source_offset,
                        resolution_scope: document.resolution_scope.clone(),
                        fragment,
                    });
                }
            }
        }
    }

    fail_on_error_diagnostics(parse_diagnostics)?;

    let scopes = scope_definitions(input, queries, loaded_fragments)?;
    let mut groups = Vec::new();
    let mut all_operations = Vec::new();
    let mut all_fragments = Vec::new();
    let mut emitted_flat_operations = HashSet::new();
    let mut emitted_flat_fragments = HashSet::new();
    for scope in scopes {
        let fragments = scope.fragment_map();
        fail_on_error_diagnostics(
            duplicate_fragment_errors(&fragments)
                .into_iter()
                .map(Into::into)
                .collect(),
        )?;

        let mut fragment_artifacts = Vec::new();
        for fragment in &scope.fragments {
            fragment_artifacts.push(build_fragment_artifact(
                &input.project,
                &input.catalog,
                &fragments,
                fragment,
            )?);
        }

        let mut operations = Vec::new();
        for query in &scope.queries {
            operations.extend(build_query_operations(
                &input.project,
                &input.catalog,
                &fragments,
                query,
                input.options,
            )?);
        }

        for operation in &operations {
            if emitted_flat_operations.insert(operation.metadata.name.clone()) {
                all_operations.push(operation.clone());
            }
        }
        for fragment in &fragment_artifacts {
            if emitted_flat_fragments.insert(fragment.metadata.name.clone()) {
                all_fragments.push(fragment.clone());
            }
        }

        groups.push(BuiltArtifactGroup {
            name: scope.name.clone(),
            scope_name: scope.name,
            imports: scope.imports,
            operations,
            fragments: fragment_artifacts,
        });
    }
    Ok(BuiltArtifacts {
        operations: all_operations,
        fragments: all_fragments,
        groups,
    })
}

fn scope_definitions(
    input: &GenerateInput,
    queries: Vec<LoadedQuery>,
    fragments: Vec<LoadedFragment>,
) -> Result<Vec<ScopeDefinitions>> {
    let mut locals = scope_locals(input);
    for query in queries {
        locals
            .entry(query.resolution_scope.clone())
            .or_insert_with(|| ScopeDefinitions::local(query.resolution_scope.clone()))
            .queries
            .push(query);
    }
    for fragment in fragments {
        locals
            .entry(fragment.resolution_scope.clone())
            .or_insert_with(|| ScopeDefinitions::local(fragment.resolution_scope.clone()))
            .fragments
            .push(fragment);
    }

    let scope_names = locals.keys().cloned().collect::<Vec<_>>();
    let mut scopes = Vec::new();
    for name in scope_names {
        let mut scope = locals
            .get(&name)
            .cloned()
            .unwrap_or_else(|| ScopeDefinitions::local(name.clone()));
        append_imported_scope_definitions(&name, &locals, &mut scope, &mut BTreeSet::new())?;
        validate_duplicate_query_names(&scope)?;
        scopes.push(scope);
    }
    Ok(scopes)
}

fn scope_locals(input: &GenerateInput) -> BTreeMap<String, ScopeDefinitions> {
    if input.project.config.resolution.is_empty() {
        let name = dsql_project::DEFAULT_RESOLUTION_SCOPE.to_string();
        return BTreeMap::from([(name.clone(), ScopeDefinitions::local(name))]);
    }

    input
        .project
        .config
        .resolution
        .iter()
        .map(|(name, config)| {
            (
                name.clone(),
                ScopeDefinitions {
                    name: name.clone(),
                    imports: config.imports.clone(),
                    queries: Vec::new(),
                    fragments: Vec::new(),
                },
            )
        })
        .collect()
}

fn append_imported_scope_definitions(
    name: &str,
    locals: &BTreeMap<String, ScopeDefinitions>,
    scope: &mut ScopeDefinitions,
    visiting: &mut BTreeSet<String>,
) -> Result<()> {
    if !visiting.insert(name.to_string()) {
        return Err(miette::miette!("cyclic resolution import involving `{name}`").into());
    }
    for import in scope.imports.clone() {
        let Some(imported) = locals.get(&import) else {
            return Err(miette::miette!(
                "resolution map `{}` imports unknown resolution map `{}`",
                scope.name,
                import
            )
            .into());
        };
        let mut effective_import = imported.clone();
        append_imported_scope_definitions(&import, locals, &mut effective_import, visiting)?;
        scope.fragments.extend(effective_import.fragments);
    }
    visiting.remove(name);
    Ok(())
}

fn validate_duplicate_query_names(scope: &ScopeDefinitions) -> Result<()> {
    let mut seen = HashMap::<&str, &LoadedQuery>::new();
    for query in &scope.queries {
        let Some(name) = query.query.key.name.as_deref() else {
            continue;
        };
        if seen.insert(name, query).is_some() {
            return Err(miette::miette!(
                "duplicate query `{}` in resolution map `{}`",
                name,
                scope.name
            )
            .into());
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct ScopeDefinitions {
    name: String,
    imports: Vec<String>,
    queries: Vec<LoadedQuery>,
    fragments: Vec<LoadedFragment>,
}

impl ScopeDefinitions {
    fn local(name: String) -> Self {
        Self {
            name,
            imports: Vec::new(),
            queries: Vec::new(),
            fragments: Vec::new(),
        }
    }

    fn fragment_map(&self) -> FragmentMap {
        let mut fragments = FragmentMap::default();
        for fragment in &self.fragments {
            fragments.insert(fragment.fragment.clone());
        }
        fragments
    }
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

    fail_on_error_diagnostics(collect_query_compiler_diagnostics(
        &checked, &linted, &planned,
    ))?;

    let query_name = query
        .query
        .key
        .name
        .as_deref()
        .ok_or_else(|| miette::miette!("anonymous queries cannot be generated"))?;
    let variables = infer_query_variable_bindings(&query.query, fragments, catalog).bindings;
    validate_variable_bindings(query_name, &variables)?;
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
            kind: DefinitionKind::Query.as_ref().to_string(),
            sql: SqlMetadata {
                dialect: SqlDialect::Postgres.as_ref().to_string(),
                text: generated.sql,
                parameters: generated
                    .parameters
                    .iter()
                    .map(|parameter| SqlParameterMetadata {
                        path: parameter.path.clone(),
                    })
                    .collect(),
                variants: generated
                    .variants
                    .iter()
                    .map(|variant| SqlVariantMetadata {
                        path: variant.path.clone(),
                        cases: variant
                            .cases
                            .iter()
                            .map(|case| SqlVariantCaseMetadata {
                                value: case.value.clone(),
                                text: case.text.clone(),
                            })
                            .collect(),
                    })
                    .collect(),
            },
            result: result_shape(catalog, plan)?,
            params: input_fields(&variables, true),
            input: input_fields(&variables, false),
            context: Vec::new(),
            dynamic_inputs: dynamic_inputs(&variables),
            policies: Vec::<PolicyMetadata>::new(),
            handoffs: Vec::<HandoffMetadata>::new(),
            fragment_spreads: query
                .query
                .selections
                .iter()
                .filter(|selection| selection.kind != SelectionKind::FragmentSpread)
                .nth(index)
                .map(|selection| operation_fragment_spreads(catalog, fragments, plan, selection))
                .unwrap_or_default(),
            source_map: source_map(project, query),
        };
        let hash = stable_hash(&facet_json::to_string(&metadata).map_err(|error| {
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

fn build_fragment_artifact(
    project: &dsql_project::Project,
    catalog: &Catalog,
    fragments: &FragmentMap,
    fragment: &LoadedFragment,
) -> Result<FragmentArtifact> {
    let checked = check_fragment_definition(&fragment.fragment, fragments, catalog);
    fail_on_error_diagnostics(collect_checked_compiler_diagnostics(&checked))?;
    let fragment_name = &fragment.fragment.key.name;
    let plan = plan_fragment_definition(&fragment.fragment, fragments, catalog)
        .ok_or_else(|| miette::miette!("fragment `{fragment_name}` cannot be planned"))?;
    let table = catalog
        .tables
        .get(plan.table.0)
        .ok_or_else(|| miette::miette!("missing fragment table for `{fragment_name}`"))?;
    let variables =
        infer_fragment_variable_bindings(&fragment.fragment, fragments, catalog).bindings;
    validate_variable_bindings(fragment_name, &variables)?;
    let metadata = FragmentMetadata {
        name: fragment_name.clone(),
        kind: DefinitionKind::Fragment.as_ref().to_string(),
        table: table.name.clone(),
        result: fragment_result_shape(catalog, &plan.selections)?,
        params: input_fields(&variables, true),
        input: input_fields(&variables, false),
        dynamic_inputs: dynamic_inputs(&variables),
        source_map: fragment_source_map(project, fragment),
    };
    let hash = stable_hash(&facet_json::to_string(&metadata).map_err(|error| {
        miette::miette!("failed to hash fragment `{}`: {error}", metadata.name)
    })?);
    Ok(FragmentArtifact {
        metadata,
        hash,
        source: source_path(project, &fragment.file),
    })
}

fn operation_fragment_spreads(
    catalog: &Catalog,
    fragments: &FragmentMap,
    plan: &QueryPlan,
    root_selection: &Selection,
) -> Vec<FragmentSpreadMetadata> {
    let mut spreads = Vec::new();
    collect_fragment_spread_metadata(
        catalog,
        fragments,
        plan.root,
        &plan.output_name,
        &root_selection.selections,
        &mut Vec::new(),
        &mut spreads,
    );
    spreads
}

fn collect_fragment_spread_metadata(
    catalog: &Catalog,
    fragments: &FragmentMap,
    table: TableId,
    result_path: &str,
    selections: &[Selection],
    visiting: &mut Vec<String>,
    spreads: &mut Vec<FragmentSpreadMetadata>,
) {
    for selection in selections {
        if selection.kind == SelectionKind::FragmentSpread {
            spreads.push(FragmentSpreadMetadata {
                path: result_path.to_string(),
                fragment: selection.name.text.clone(),
            });
            let Some(fragment) = fragments.fragment(&selection.name.text) else {
                continue;
            };
            if visiting.iter().any(|name| name == &fragment.key.name) {
                continue;
            }
            visiting.push(fragment.key.name.clone());
            collect_fragment_spread_metadata(
                catalog,
                fragments,
                table,
                result_path,
                &fragment.selections,
                visiting,
                spreads,
            );
            visiting.pop();
            continue;
        }

        if let FieldCheckResult::Relation(relation) =
            catalog.check_field(table, &selection.name.text)
        {
            let child_name = selection
                .alias
                .as_ref()
                .map_or_else(|| relation.name.to_string(), |alias| alias.text.clone());
            collect_fragment_spread_metadata(
                catalog,
                fragments,
                relation.table.id,
                &join_path(result_path, &child_name),
                &selection.selections,
                visiting,
                spreads,
            );
        }
    }
}

fn validate_variable_bindings(query_name: &str, variables: &[VariableBinding]) -> Result<()> {
    let mut anonymous_paths = HashMap::<&str, &VariableBinding>::new();
    for binding in variables.iter().filter(|binding| binding.name.is_none()) {
        if let Some(previous) = anonymous_paths.insert(&binding.path, binding) {
            return Err(miette::miette!(
                "query `{query_name}` has multiple anonymous variables for `{}`; name one of them to disambiguate",
                previous.path
            )
            .into());
        }
    }
    Ok(())
}

fn manifest_from_written_artifacts(
    operations: &[WrittenOperationArtifact],
    fragments: &[WrittenFragmentArtifact],
) -> BuildManifest {
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
        fragments: fragments
            .iter()
            .map(|fragment| FragmentManifestEntry {
                name: fragment.metadata.name.clone(),
                kind: fragment.metadata.kind.clone(),
                path: fragment.reference.path.clone(),
                hash: fragment.hash.clone(),
                source: fragment.source.clone(),
            })
            .collect(),
    }
}

fn fail_on_error_diagnostics(mut diagnostics: Vec<CompilerDiagnostic>) -> Result<()> {
    diagnostics.sort_by_key(|diagnostic| {
        let range = DsqlDiagnostic::range(diagnostic);
        (range.start, range.end)
    });
    let errors = diagnostics
        .into_iter()
        .filter(|diagnostic| DsqlDiagnostic::severity(diagnostic) == Severity::Error)
        .collect::<Vec<_>>();
    if errors.is_empty() {
        return Ok(());
    }

    let mut details = String::new();
    for diagnostic in errors {
        details.push_str(&format!(
            "\n{:?} {:?} {}..{}: {}",
            DsqlDiagnostic::source(&diagnostic),
            DsqlDiagnostic::code(&diagnostic),
            DsqlDiagnostic::range(&diagnostic).start,
            DsqlDiagnostic::range(&diagnostic).end,
            diagnostic
        ));
    }
    Err(GenerateError::LanguageDiagnostics { details })
}

fn result_shape(catalog: &Catalog, plan: &QueryPlan) -> Result<ResultShape> {
    let mut fields = Vec::new();
    collect_result_fields(
        catalog,
        "",
        &plan.output_name,
        &plan.selections,
        ResultFieldKind::Array,
        false,
        &mut fields,
    )?;
    Ok(ResultShape { fields })
}

fn fragment_result_shape(catalog: &Catalog, selection: &SelectionPlan) -> Result<ResultShape> {
    let mut fields = Vec::new();
    for item in &selection.items {
        collect_result_item_fields(catalog, selection.table, "", item, &mut fields)?;
    }
    Ok(ResultShape { fields })
}

fn collect_result_fields(
    catalog: &Catalog,
    parent_path: &str,
    name: &str,
    selection: &SelectionPlan,
    kind: ResultFieldKind,
    nullable: bool,
    fields: &mut Vec<ResultField>,
) -> Result<()> {
    let path = join_path(parent_path, name);
    fields.push(ResultField {
        path: path.clone(),
        name: name.to_string(),
        parent_path: parent_path.to_string(),
        kind: kind.as_ref().to_string(),
        data_type: ResultDataType::Object.as_ref().to_string(),
        nullable,
    });

    for item in &selection.items {
        collect_result_item_fields(catalog, selection.table, &path, item, fields)?;
    }
    Ok(())
}

fn collect_result_item_fields(
    catalog: &Catalog,
    current_table: TableId,
    parent_path: &str,
    item: &SelectionPlanItem,
    fields: &mut Vec<ResultField>,
) -> Result<()> {
    match item {
        SelectionPlanItem::Projection(projection) => {
            let column = catalog
                .column_by_id(projection.column)
                .ok_or_else(|| miette::miette!("missing projected column"))?;
            fields.push(ResultField {
                path: join_path(parent_path, &projection.output_name),
                name: projection.output_name.clone(),
                parent_path: parent_path.to_string(),
                kind: ResultFieldKind::Scalar.as_ref().to_string(),
                data_type: column.data_type.as_str().to_string(),
                nullable: !column.not_null,
            });
        }
        SelectionPlanItem::Relation(relation) => {
            let foreign_key = catalog
                .foreign_key_by_id(relation.foreign_key)
                .ok_or_else(|| miette::miette!("missing relation foreign key"))?;
            let cardinality = catalog
                .relation_cardinality(current_table, relation.table, foreign_key)
                .ok_or_else(|| miette::miette!("invalid relation foreign key"))?;
            let kind = match cardinality {
                dsql_core::RelationCardinality::Collection => ResultFieldKind::Array,
                dsql_core::RelationCardinality::Singular => ResultFieldKind::Object,
            };
            let nullable = match cardinality {
                dsql_core::RelationCardinality::Collection => false,
                dsql_core::RelationCardinality::Singular => {
                    catalog.relation_is_nullable(current_table, relation.table, foreign_key)
                        || singular_relation_can_be_absent(&relation.selections.clauses)
                }
            };
            collect_result_fields(
                catalog,
                parent_path,
                &relation.output_name,
                &relation.selections,
                kind,
                nullable,
                fields,
            )?;
        }
    }
    Ok(())
}

fn singular_relation_can_be_absent(clauses: &SelectionClauses) -> bool {
    clauses.filter.is_some()
        || clauses.offset.is_some()
        || matches!(
            clauses.limit,
            Some(SqlValue::Literal(0) | SqlValue::Parameter(_))
        )
}

fn input_fields(variables: &[VariableBinding], top_level: bool) -> Vec<InputField> {
    variables
        .iter()
        .filter(|binding| {
            (top_level && is_params_path(&binding.path))
                || (!top_level && is_input_path(&binding.path))
        })
        .map(|binding| InputField {
            path: binding.path.clone(),
            data_type: binding.data_type.as_str().to_string(),
            enum_values: binding.enum_values.clone(),
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
                    .unwrap_or(InputPathSegment::Value.as_ref())
                    .to_string()
            }),
            kind: binding.role.as_ref().to_string(),
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
            start: query.source_offset + query.query.range.start,
            end: query.source_offset + query.query.range.end,
        },
    }]
}

fn fragment_source_map(
    project: &dsql_project::Project,
    fragment: &LoadedFragment,
) -> Vec<SourceMapEntry> {
    vec![SourceMapEntry {
        id: fragment.fragment.key.name.clone(),
        file: source_path(project, &fragment.file),
        range: SourceRange {
            start: fragment.source_offset + fragment.fragment.range.start,
            end: fragment.source_offset + fragment.fragment.range.end,
        },
    }]
}

pub(crate) fn project_root(project: &dsql_project::Project) -> PathBuf {
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

#[derive(Clone, Debug)]
pub(crate) struct LoadedQuery {
    file: PathBuf,
    source_offset: u32,
    resolution_scope: String,
    query: QueryRecord,
}

#[derive(Clone, Debug)]
pub(crate) struct LoadedFragment {
    file: PathBuf,
    source_offset: u32,
    resolution_scope: String,
    fragment: FragmentRecord,
}
