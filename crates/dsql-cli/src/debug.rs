//! Debug introspection over the project bowl, compiled into debug builds
//! only: drive the same request/response services the LSP uses, from the
//! command line, plus dumps of what the bowl derived — so editor problems
//! reproduce without an editor in the loop.

use bowl::{Bowl, Entity, Query, Singleton};

use dsql_core::entities::definition::DefDecl;
use dsql_core::entities::field_selection::FieldSel;
use dsql_core::entities::fragment_spread::SpreadDecl;
use dsql_core::entities::variable::VariableUse;
use dsql_core::facts::{BelongsToFile, DiagnosticsDemand, NodeKey, VariablesDemand};
use dsql_core::service::{
    CompletionList, CompletionRequest, DefinitionRequest, DefinitionTarget, HoverInfo,
    HoverRequest, Position, semantic_tokens,
};
use dsql_core::source::{
    BelongsToHost, DsqlDocument, EmbeddingHost, FilePath, ResolutionScope, ScopeImports,
    SourceOffset, SourceText,
};
use dsql_project::{Project, ProjectError, open_project_bowl};

use crate::commands::Outcome;

/// A session bowl configured like an LSP session: language, project
/// contents, and the demand markers the editor path inserts.
async fn session_bowl(project: &Project) -> Result<Bowl, ProjectError> {
    let bowl = open_project_bowl(project).await?;
    bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
        .await;
    bowl.insert((Singleton::<VariablesDemand>::new(), VariablesDemand))
        .await;
    Ok(bowl)
}

/// Resolves a user-given path against the bowl's file entities: exact
/// match first, then unique suffix match — so relative paths work from
/// anywhere inside the project.
async fn resolve_file(bowl: &Bowl, file: &str) -> Option<(Entity, String)> {
    let paths = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
    let paths = paths.collect();
    if let Some((entity, path)) = paths.iter().find(|(_, path)| path.0 == file) {
        return Some((*entity, path.0.clone()));
    }
    let suffix: Vec<_> = paths
        .iter()
        .filter(|(_, path)| path.0.ends_with(file))
        .collect();
    if let [(entity, path)] = suffix.as_slice() {
        return Some((*entity, path.0.clone()));
    }
    eprintln!(
        "file `{file}` matches {} of {} bowl files",
        suffix.len(),
        paths.len()
    );
    None
}

/// The lowered facts whose spans contain `offset` in `file` — what the
/// hover candidates have to work with at that position.
async fn print_facts_at(bowl: &Bowl, file: Entity, offset: usize) {
    let mut hits = Vec::new();

    let defs = bowl
        .scoop::<Query<(Entity, &DefDecl, &BelongsToFile)>>()
        .await;
    for (_, decl, of) in defs.collect() {
        if of.0 == file && decl.name_span.start <= offset && offset < decl.name_span.end {
            hits.push(format!(
                "def `{}` name at {}..{}",
                decl.name, decl.name_span.start, decl.name_span.end
            ));
        }
    }
    let fields = bowl
        .scoop::<Query<(Entity, &FieldSel, &NodeKey, &BelongsToFile)>>()
        .await;
    for (_, field, _, of) in fields.collect() {
        if of.0 == file && field.name_span.start <= offset && offset < field.name_span.end {
            hits.push(format!(
                "field `{}` name at {}..{}",
                field.name, field.name_span.start, field.name_span.end
            ));
        }
    }
    let spreads = bowl
        .scoop::<Query<(Entity, &SpreadDecl, &BelongsToFile)>>()
        .await;
    for (_, spread, of) in spreads.collect() {
        if of.0 == file && spread.name_span.start <= offset && offset < spread.name_span.end {
            hits.push(format!(
                "spread `{}` name at {}..{}",
                spread.name, spread.name_span.start, spread.name_span.end
            ));
        }
    }
    let variables = bowl
        .scoop::<Query<(Entity, &VariableUse, &BelongsToFile)>>()
        .await;
    for (_, variable, of) in variables.collect() {
        if of.0 == file && variable.0.span.start <= offset && offset < variable.0.span.end {
            hits.push(format!(
                "variable at {}..{}",
                variable.0.span.start, variable.0.span.end
            ));
        }
    }

    if hits.is_empty() {
        println!("facts at offset {offset}: none");
    } else {
        for hit in hits {
            println!("facts at offset {offset}: {hit}");
        }
    }
}

pub async fn hover(file: &str, offset: usize) -> Outcome {
    let project = Project::load().await?;
    {
        let bowl = session_bowl(&project).await?;
        let Some((entity, path)) = resolve_file(&bowl, file).await else {
            return Ok(false);
        };
        println!("file: {path} (entity {})", entity.raw());
        print_facts_at(&bowl, entity, offset).await;

        let info = bowl
            .insert((HoverRequest, FilePath(path), Position { offset }))
            .await
            .bind()
            .take::<HoverInfo>()
            .await;
        match info {
            Ok(info) => println!("hover ({}): {}", info.priority, info.text),
            Err(error) => println!("hover: <no answer: {error:?}>"),
        }
        Ok(true)
    }
}

pub async fn goto(file: &str, offset: usize) -> Outcome {
    let project = Project::load().await?;
    {
        let bowl = session_bowl(&project).await?;
        let Some((_, path)) = resolve_file(&bowl, file).await else {
            return Ok(false);
        };
        let target = bowl
            .insert((DefinitionRequest, FilePath(path), Position { offset }))
            .await
            .bind()
            .take::<DefinitionTarget>()
            .await;
        match target {
            Ok(target) => {
                let paths = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
                let target_path = paths
                    .collect()
                    .into_iter()
                    .find(|(entity, _)| *entity == target.file)
                    .map(|(_, path)| path.0.clone())
                    .unwrap_or_else(|| format!("<entity {}>", target.file.raw()));
                println!(
                    "definition: {target_path}:{}..{}",
                    target.span.start, target.span.end
                );
            }
            Err(error) => println!("definition: <no answer: {error:?}>"),
        }
        Ok(true)
    }
}

pub async fn complete(file: &str, offset: usize) -> Outcome {
    let project = Project::load().await?;
    {
        let bowl = session_bowl(&project).await?;
        let Some((_, path)) = resolve_file(&bowl, file).await else {
            return Ok(false);
        };
        let list = bowl
            .insert((CompletionRequest, FilePath(path), Position { offset }))
            .await
            .bind()
            .take::<CompletionList>()
            .await;
        match list {
            Ok(list) => {
                for item in &list.0 {
                    println!(
                        "{:?} {}{}",
                        item.kind,
                        item.label,
                        item.detail
                            .as_deref()
                            .map(|detail| format!(" — {detail}"))
                            .unwrap_or_default()
                    );
                }
                if list.0.is_empty() {
                    println!("no completions");
                }
            }
            Err(error) => println!("completions: <no answer: {error:?}>"),
        }
        Ok(true)
    }
}

pub async fn tokens(file: &str) -> Outcome {
    let project = Project::load().await?;
    {
        let bowl = session_bowl(&project).await?;
        let Some((_, path)) = resolve_file(&bowl, file).await else {
            return Ok(false);
        };
        let tokens = semantic_tokens(&bowl, path).await;
        for token in &tokens {
            println!("{:?} {}..{}", token.kind, token.span.start, token.span.end);
        }
        if tokens.is_empty() {
            println!("no tokens");
        }
        Ok(true)
    }
}

/// Which files (and derived regions) belong to which resolution scope,
/// plus the scope import graph.
pub async fn resolution() -> Outcome {
    let project = Project::load().await?;
    {
        let bowl = session_bowl(&project).await?;

        let files = bowl
            .scoop::<Query<(
                Entity,
                &FilePath,
                &ResolutionScope,
                Option<&DsqlDocument>,
                Option<&EmbeddingHost>,
            )>>()
            .await;
        for (entity, path, scope, document, host) in files.collect() {
            let kind = match (document.is_some(), host.is_some()) {
                (_, true) => "host",
                (true, _) => "document",
                _ => "?",
            };
            println!(
                "{kind} {} scope `{}` (entity {})",
                path.0,
                scope.0,
                entity.raw()
            );
        }

        let regions = bowl
            .scoop::<Query<(
                Entity,
                &BelongsToHost,
                &SourceOffset,
                &ResolutionScope,
                &SourceText,
            )>>()
            .await;
        for (entity, host, offset, scope, text) in regions.collect() {
            println!(
                "region entity {} of host entity {} at offset {} scope `{}` ({} bytes)",
                entity.raw(),
                host.0.raw(),
                offset.0,
                scope.0,
                text.to_text().len()
            );
        }

        let imports = bowl.scoop::<Query<(Entity, &ScopeImports)>>().await;
        for (_, imports) in imports.collect() {
            for (scope, imported) in &imports.0 {
                println!("scope `{scope}` imports {imported:?}");
            }
        }
        Ok(true)
    }
}

/// Prints the engine's explain report for a system, after a settle with
/// the usual demand markers — how many invocations its joins currently
/// plan, and how many are memo-current.
pub async fn explain(system: &str) -> Outcome {
    let project = Project::load().await?;
    let bowl = session_bowl(&project).await?;
    // Force a settle so the report reflects steady state.
    let _ = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
    let report = bowl.explain(system).await;
    println!("{report:#?}");
    Ok(true)
}
