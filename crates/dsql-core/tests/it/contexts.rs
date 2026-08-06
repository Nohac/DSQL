//! Explicit trusted-context declarations, resolution, and editor services.

use std::collections::BTreeMap;

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::entities::context::{
    ContextDecl, ContextIndex, ContextUseResolution, ResolvedContextUse,
};
use dsql_core::entities::definition::DefIndex;
use dsql_core::entities::document::ParsedFile;
use dsql_core::entities::expression::Sigil;
use dsql_core::entities::variable::{VariableBinding, VariableProblem, VariableUse};
use dsql_core::facts::{BelongsToFile, Diagnostic, Span, arm_editor_demands};
use dsql_core::language_bowl;
use dsql_core::service::{
    CompletionList, CompletionRequest, DefinitionRequest, DefinitionTarget, HoverInfo,
    HoverRequest, Position,
};
use dsql_core::source::{
    FilePath, ResolutionScope, ScopeImports, SourceKind, arm_analysis_residency,
    insert_embedding_source, insert_source, insert_source_scoped,
};

use crate::variables::render_bindings;
use crate::{native_enum_catalog, render_diagnostic_facts, set_source_text};

const PATH: &str = "contexts.dsql";

async fn context_bowl(source: &str) -> Bowl {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, native_enum_catalog()).await;
    arm_editor_demands(&bowl).await;
    insert_source(&bowl, PATH, source).await;
    bowl
}

async fn render_context_pipeline(bowl: &Bowl) -> String {
    let files = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
    let paths = files
        .collect()
        .into_iter()
        .map(|(entity, path)| (entity, path.0.clone()))
        .collect::<BTreeMap<_, _>>();
    let path_of = |file: Entity| {
        paths
            .get(&file)
            .cloned()
            .unwrap_or_else(|| format!("<missing:{file:?}>"))
    };
    let mut lines = Vec::new();

    let parsed = bowl.scoop::<Query<(Entity, &ParsedFile)>>().await;
    for (file, parsed) in parsed.collect() {
        lines.push(format!(
            "parsed {} bytes={}",
            path_of(file),
            parsed.source.len()
        ));
    }

    let declarations = bowl
        .scoop::<Query<(Entity, &ContextDecl, &BelongsToFile)>>()
        .await;
    for (_, declaration, file) in declarations.collect() {
        lines.push(format!(
            "declaration {} {} name=[{}..{}] type=[{}..{}]",
            path_of(file.0),
            declaration.name,
            declaration.name_span.start,
            declaration.name_span.end,
            declaration.type_span.start,
            declaration.type_span.end,
        ));
    }

    let indices = bowl.scoop::<Query<(Entity, &ContextIndex)>>().await;
    let indices = indices.collect();
    lines.push(format!("indices {}", indices.len()));
    for (_, index) in indices {
        for entry in &index.entries {
            let contract = entry.contract.as_ref().map_or_else(
                || "invalid".to_string(),
                |contract| {
                    format!(
                        "{}{}",
                        contract.data_type.as_str(),
                        if contract.collection { "[]" } else { "" }
                    )
                },
            );
            lines.push(format!(
                "index {}:{} {} name=[{}..{}] type=[{}..{}] contract={contract}",
                entry.scope,
                entry.name,
                entry.file_path,
                entry.name_span.start,
                entry.name_span.end,
                entry.type_span.start,
                entry.type_span.end,
            ));
        }
    }
    let definition_indices = bowl.scoop::<Query<(Entity, &DefIndex)>>().await;
    lines.push(format!("definition-indices {}", definition_indices.len()));

    let uses = bowl
        .scoop::<Query<(Entity, &VariableUse, &BelongsToFile)>>()
        .await;
    for (_, variable, file) in uses.collect() {
        if variable.sigil() == Sigil::Context {
            lines.push(format!(
                "use {} {} [{}..{}]",
                path_of(file.0),
                variable.0.name.as_deref().unwrap_or("<anonymous>"),
                variable.0.span.start,
                variable.0.span.end,
            ));
        }
    }

    let resolved = bowl
        .scoop::<Query<(Entity, &ResolvedContextUse, &BelongsToFile)>>()
        .await;
    for (_, resolved, file) in resolved.collect() {
        let outcome = match &resolved.resolution {
            ContextUseResolution::Resolved {
                name_span,
                contract,
                ..
            } => format!(
                "resolved declaration=[{}..{}] contract={}",
                name_span.start,
                name_span.end,
                contract.data_type.as_str(),
            ),
            ContextUseResolution::Unknown => "unknown".to_string(),
            ContextUseResolution::Ambiguous { providers } => {
                format!("ambiguous providers={}", providers.join(","))
            }
            ContextUseResolution::Invalid => "invalid".to_string(),
        };
        lines.push(format!(
            "resolved-use {} {} [{}..{}] {outcome}",
            path_of(file.0),
            resolved.name,
            resolved.span.start,
            resolved.span.end,
        ));
    }

    let problems = bowl
        .scoop::<Query<(Entity, &VariableProblem, &Span, &BelongsToFile)>>()
        .await;
    for (_, _, span, file) in problems.collect() {
        lines.push(format!(
            "variable-problem {} [{}..{}]",
            path_of(file.0),
            span.start,
            span.end,
        ));
    }

    let diagnostics = bowl
        .scoop::<Query<(Entity, &Diagnostic, &Span, &BelongsToFile)>>()
        .await;
    for (_, diagnostic, span, file) in diagnostics.collect() {
        lines.push(format!(
            "diagnostic {} [{}..{}] {}",
            path_of(file.0),
            span.start,
            span.end,
            diagnostic.0,
        ));
    }

    lines.sort();
    lines.join("\n")
}

#[tokio::test]
async fn declarations_drive_bindings_hover_definition_and_completion() {
    let source = indoc::indoc! {r#"
        context {
          status: public::status
          statuses: public::status[]
          direction: text
          page_size: int
        }

        query ContextQuery {
          enum_records(
            where .status == $:status
              and .status in $:statuses
            order by status $:direction
            limit $:page_size
          ) {
            status
          }
        }
    "#};
    let bowl = context_bowl(source).await;

    let declarations = bowl.scoop::<Query<(Entity, &ContextDecl)>>().await;
    assert_eq!(declarations.collect().len(), 4);
    let indices = bowl.scoop::<Query<(Entity, &ContextIndex)>>().await;
    assert_eq!(indices.collect().len(), 1);
    assert_eq!(render_diagnostic_facts(&bowl).await, "");

    insta::assert_snapshot!(render_bindings(&bowl).await);

    let use_offset = source.find("$:status").expect("context use") + 2;
    let hover = bowl
        .insert((
            HoverRequest,
            FilePath(PATH.to_string()),
            Position { offset: use_offset },
        ))
        .await
        .bind()
        .take::<HoverInfo>()
        .await
        .expect("hover answered");
    insta::assert_snapshot!(hover.text.as_str());

    let target = bowl
        .insert((
            DefinitionRequest,
            FilePath(PATH.to_string()),
            Position { offset: use_offset },
        ))
        .await
        .bind()
        .take::<DefinitionTarget>()
        .await
        .expect("definition answered");
    assert!(matches!(target.as_ref(), DefinitionTarget::Source { .. }));
    let DefinitionTarget::Source { span, .. } = target.as_ref() else {
        return;
    };
    let declaration_offset = source.find("status: public").expect("declaration");
    assert_eq!(
        *span,
        Span {
            start: declaration_offset,
            end: declaration_offset + "status".len(),
        }
    );

    let completion_offset = source.find("$:status").expect("context use") + 2;
    let completion = bowl
        .insert((
            CompletionRequest,
            FilePath(PATH.to_string()),
            Position {
                offset: completion_offset,
            },
        ))
        .await
        .bind()
        .take::<CompletionList>()
        .await
        .expect("completion answered");
    let rendered = completion
        .items
        .iter()
        .map(|item| {
            format!(
                "{}: {}",
                item.label,
                item.detail.as_deref().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!(rendered);
}

#[tokio::test]
async fn invalid_declarations_unknown_uses_and_mismatches_are_reported() {
    let source = indoc::indoc! {r#"
        context {
          page_size: int
          unqualified_enum: status
          provider_array: public::_status
        }

        query InvalidContext {
          enum_records(
            where .status == $:page_size
              and .status == $:missing
          ) {
            status
          }
        }
    "#};
    let bowl = context_bowl(source).await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
    let bindings = bowl.scoop::<Query<(Entity, &VariableBinding)>>().await;
    assert!(bindings.collect().is_empty());
}

#[tokio::test]
async fn context_visibility_and_collisions_follow_scope_imports() {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, Catalog::hardcoded()).await;
    arm_editor_demands(&bowl).await;
    bowl.insert((
        Singleton::<ScopeImports>::new(),
        ScopeImports(BTreeMap::from([
            ("frontend".to_string(), vec!["shared".to_string()]),
            ("shared".to_string(), Vec::new()),
        ])),
    ))
    .await;
    let shared = insert_source_scoped(
        &bowl,
        "shared.dsql",
        "context { tenant_id: uuid }\n",
        ResolutionScope("shared".to_string()),
        SourceKind::Dsql,
    )
    .await;
    insert_source_scoped(
        &bowl,
        "frontend.dsql",
        "query Q { public::users(where .id == $:tenant_id) { id } }\n",
        ResolutionScope("frontend".to_string()),
        SourceKind::Dsql,
    )
    .await;
    assert_eq!(render_diagnostic_facts(&bowl).await, "");

    let target = bowl
        .insert((
            DefinitionRequest,
            FilePath("frontend.dsql".to_string()),
            Position {
                offset: "query Q { public::users(where .id == $:".len(),
            },
        ))
        .await
        .bind()
        .take::<DefinitionTarget>()
        .await
        .expect("imported context definition answered");
    assert!(matches!(
        target.as_ref(),
        DefinitionTarget::Source { file, .. } if *file == shared
    ));

    insert_source_scoped(
        &bowl,
        "local.dsql",
        "context { tenant_id: uuid }\n",
        ResolutionScope("frontend".to_string()),
        SourceKind::Dsql,
    )
    .await;
    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn context_declaration_collision_rules_are_reported() {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, Catalog::hardcoded()).await;
    arm_editor_demands(&bowl).await;
    bowl.insert((
        Singleton::<ScopeImports>::new(),
        ScopeImports(BTreeMap::from([
            (
                "frontend".to_string(),
                vec!["shared_a".to_string(), "shared_b".to_string()],
            ),
            ("shared_a".to_string(), Vec::new()),
            ("shared_b".to_string(), Vec::new()),
            ("duplicate".to_string(), Vec::new()),
        ])),
    ))
    .await;
    for (path, source, scope) in [
        (
            "duplicate-a.dsql",
            "context { local_key: text }\n",
            "duplicate",
        ),
        (
            "duplicate-b.dsql",
            "context { local_key: text }\n",
            "duplicate",
        ),
        ("shared-a.dsql", "context { tenant_id: uuid }\n", "shared_a"),
        ("shared-b.dsql", "context { tenant_id: uuid }\n", "shared_b"),
        (
            "frontend.dsql",
            "query Q { public::users(where .id == $:tenant_id) { id } }\n",
            "frontend",
        ),
    ] {
        insert_source_scoped(
            &bowl,
            path,
            source,
            ResolutionScope(scope.to_string()),
            SourceKind::Dsql,
        )
        .await;
    }

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}

#[tokio::test]
async fn declaration_edits_rederive_context_contracts() {
    let source = indoc::indoc! {r#"
        context { status: int }
        query Q { enum_records(where .status == $:status) { status } }
    "#};
    let bowl = context_bowl(source).await;
    let file = bowl
        .scoop::<Query<(Entity, &FilePath)>>()
        .await
        .collect()
        .into_iter()
        .find_map(|(entity, path)| (path.0 == PATH).then_some(entity))
        .expect("context source exists");

    let before = render_diagnostic_facts(&bowl).await;
    set_source_text(
        &bowl,
        file,
        source.replace("status: int", "status: public::status"),
    )
    .await;
    let after = render_diagnostic_facts(&bowl).await;
    let bindings = render_bindings(&bowl).await;

    insta::assert_snapshot!(format!(
        "before:\n{before}\n\nafter:\n{after}\n\nbindings:\n{bindings}"
    ));
}

#[tokio::test]
async fn context_pipeline_tracks_multifile_moves_removal_and_reinsertion() {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, Catalog::hardcoded()).await;
    arm_editor_demands(&bowl).await;
    bowl.insert((
        Singleton::<ScopeImports>::new(),
        ScopeImports(BTreeMap::from([
            ("frontend".to_string(), vec!["shared".to_string()]),
            ("shared".to_string(), Vec::new()),
        ])),
    ))
    .await;
    let declaration_source = "context { tenant_id: text }\n";
    let use_source = "query Q { public::users(where .id == $:tenant_id) { id } }\n";
    let declaration_file = insert_source_scoped(
        &bowl,
        "definitions.dsql",
        declaration_source,
        ResolutionScope("shared".to_string()),
        SourceKind::Dsql,
    )
    .await;
    insert_source_scoped(
        &bowl,
        "query.dsql",
        use_source,
        ResolutionScope("frontend".to_string()),
        SourceKind::Dsql,
    )
    .await;

    let initial = render_context_pipeline(&bowl).await;
    assert!(
        initial.contains("trusted context `context.tenant_id` is declared as `text`"),
        "initial mismatch must diagnose:\n{initial}"
    );

    set_source_text(&bowl, declaration_file, format!("\n{declaration_source}")).await;
    let declaration_moved = render_context_pipeline(&bowl).await;
    assert!(
        declaration_moved.contains("index shared:tenant_id definitions.dsql name=[11..20]"),
        "the index must carry the moved declaration span:\n{declaration_moved}"
    );

    let use_file = bowl
        .scoop::<Query<(Entity, &FilePath)>>()
        .await
        .collect()
        .into_iter()
        .find_map(|(entity, path)| (path.0 == "query.dsql").then_some(entity))
        .expect("use source exists");
    set_source_text(&bowl, use_file, format!("\n{use_source}")).await;
    let use_moved = render_context_pipeline(&bowl).await;
    let moved_offset = use_source.find("$:tenant_id").expect("context use") + 1;
    assert!(
        use_moved.contains(&format!(
            "variable-problem query.dsql [{}..{}]",
            moved_offset,
            moved_offset + "$:tenant_id".len(),
        )),
        "the mismatch must move with the use:\n{use_moved}"
    );

    set_source_text(&bowl, declaration_file, "\ncontext { tenant_id: uuid }\n").await;
    let repaired = render_context_pipeline(&bowl).await;
    assert!(
        repaired.contains("index shared:tenant_id definitions.dsql")
            && !repaired.contains("variable-problem")
            && !repaired.contains("diagnostic"),
        "repairing the declaration contract must clear the mismatch:\n{repaired}"
    );

    set_source_text(&bowl, declaration_file, "").await;
    let emptied = render_context_pipeline(&bowl).await;
    assert!(
        emptied.contains("resolved-use query.dsql tenant_id") && emptied.contains(" unknown"),
        "emptying the declaration file must make the use unknown:\n{emptied}"
    );
    assert!(
        !emptied.contains("index shared:tenant_id"),
        "the singleton index must drop declarations removed by text edit:\n{emptied}"
    );

    set_source_text(&bowl, declaration_file, declaration_source).await;
    let restored = render_context_pipeline(&bowl).await;
    assert!(
        restored.contains("trusted context `context.tenant_id` is declared as `text`"),
        "restoring the declaration must restore the mismatch:\n{restored}"
    );

    bowl.entity(declaration_file).despawn().await;
    let despawned = render_context_pipeline(&bowl).await;
    assert!(
        despawned.contains("resolved-use query.dsql tenant_id") && despawned.contains(" unknown"),
        "despawning the declaration file must make the use unknown:\n{despawned}"
    );
    assert!(
        despawned.lines().any(|line| line == "definition-indices 1")
            && despawned.lines().any(|line| line == "indices 1")
            && !despawned.contains("index shared:tenant_id"),
        "the shared singleton index must survive with no stale entry:\n{despawned}"
    );

    let reinserted_file = insert_source_scoped(
        &bowl,
        "definitions.dsql",
        declaration_source,
        ResolutionScope("shared".to_string()),
        SourceKind::Dsql,
    )
    .await;
    let reinserted = render_context_pipeline(&bowl).await;
    assert!(
        reinserted.contains("trusted context `context.tenant_id` is declared as `text`"),
        "reinsertion must restore the mismatch:\n{reinserted}"
    );

    // Queue removal and replacement before the next settle. The semantic
    // index contents are identical, but the navigation target must move to
    // the replacement source entity.
    bowl.entity(reinserted_file).despawn().await;
    let replacement_file = insert_source_scoped(
        &bowl,
        "definitions.dsql",
        declaration_source,
        ResolutionScope("shared".to_string()),
        SourceKind::Dsql,
    )
    .await;
    let replaced = render_context_pipeline(&bowl).await;
    assert!(
        replaced.lines().any(|line| line == "definition-indices 1")
            && replaced.lines().any(|line| line == "indices 1"),
        "batched source replacement must preserve both global indices:\n{replaced}"
    );
    let target = bowl
        .insert((
            DefinitionRequest,
            FilePath("query.dsql".to_string()),
            Position {
                offset: moved_offset + 2,
            },
        ))
        .await
        .bind()
        .take::<DefinitionTarget>()
        .await
        .expect("replacement context definition answered");
    assert!(matches!(
        target.as_ref(),
        DefinitionTarget::Source { file, span }
            if *file == replacement_file
                && *span == Span { start: 10, end: 19 }
    ));

    insta::assert_snapshot!(format!(
        "initial:\n{initial}\n\ndeclaration moved:\n{declaration_moved}\n\nuse moved:\n{use_moved}\n\nrepaired:\n{repaired}\n\nemptied:\n{emptied}\n\nrestored:\n{restored}\n\ndespawned:\n{despawned}\n\nreinserted:\n{reinserted}\n\nbatched replacement:\n{replaced}"
    ));
}

#[tokio::test]
async fn context_pipeline_tracks_moves_under_analysis_residency() {
    let bowl = language_bowl().await;
    arm_analysis_residency(&bowl).await;
    insert_catalog(&bowl, Catalog::hardcoded()).await;
    arm_editor_demands(&bowl).await;
    let source =
        "context { tenant_id: text }\nquery Q { public::users(where .id == $:tenant_id) { id } }\n";
    let file = insert_source(&bowl, "batch.dsql", source).await;
    let initial = render_context_pipeline(&bowl).await;
    assert!(initial.contains("variable-problem batch.dsql"));

    set_source_text(&bowl, file, format!("\n{source}")).await;
    let moved = render_context_pipeline(&bowl).await;
    let moved_offset = source.find("$:tenant_id").expect("context use") + 1;
    assert!(
        moved.contains(&format!(
            "variable-problem batch.dsql [{}..{}]",
            moved_offset,
            moved_offset + "$:tenant_id".len(),
        )),
        "batch-mode diagnostics must follow rehydrated source:\n{moved}"
    );
}

#[tokio::test]
async fn embedded_context_declarations_are_rejected() {
    let bowl = language_bowl().await;
    insert_catalog(&bowl, Catalog::hardcoded()).await;
    arm_editor_demands(&bowl).await;
    insert_embedding_source(
        &bowl,
        "src/query.ts",
        "export const query = dsql`context { tenant_id: uuid } query Q { public::users(where .id == $:tenant_id) { id } }`;\n",
        "typescript",
    )
    .await;

    insta::assert_snapshot!(render_diagnostic_facts(&bowl).await);
}
