//! Explicit trusted-context declarations, resolution, and editor services.

use std::collections::BTreeMap;

use bowl::{Bowl, Entity, Query, Singleton};
use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::entities::context::{ContextDecl, ContextIndex};
use dsql_core::entities::variable::VariableBinding;
use dsql_core::facts::{Span, arm_editor_demands};
use dsql_core::language_bowl;
use dsql_core::service::{
    CompletionList, CompletionRequest, DefinitionRequest, DefinitionTarget, HoverInfo,
    HoverRequest, Position,
};
use dsql_core::source::{
    FilePath, ResolutionScope, ScopeImports, SourceKind, insert_embedding_source, insert_source,
    insert_source_scoped,
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
