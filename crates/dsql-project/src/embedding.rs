//! Embedding configuration: validates named extraction providers at the
//! project boundary and builds the fingerprinted registry installed in the
//! bowl. Physical file paths select a resolver; they never select a provider
//! by extension.

use std::collections::{BTreeMap, BTreeSet};

use dsql_core::embedding::{
    ExtractionRegistry, ExtractionStrategy, compile_embedding_pattern, default_typescript_pattern,
};
use dsql_core::source::SourceKind;

use super::config::{EmbeddingConfig, EmbeddingStrategy, Project, ProjectError, Result};

/// Builds every named extraction provider referenced by project documents.
/// The built-in `typescript` resolver uses the default tagged-template regex
/// when it has no explicit `[embedding.typescript]` section.
pub fn extraction_registry(project: &Project) -> Result<ExtractionRegistry> {
    let mut resolvers = BTreeSet::new();
    if project.config.resolution.is_empty() {
        for document in &project.config.documents {
            resolvers.insert(document.resolver.as_str());
        }
    } else {
        for scope in project.config.resolution.values() {
            for document in &scope.documents {
                resolvers.insert(document.resolver.as_str());
            }
        }
    }

    let mut providers = BTreeMap::new();
    for resolver in resolvers {
        if resolver == SourceKind::DSQL_RESOLVER {
            continue;
        }
        let strategy = if let Some(config) = project.config.embedding.get(resolver) {
            strategy_for(resolver, config)?
        } else if resolver == "typescript" {
            ExtractionStrategy::Regex {
                pattern: default_typescript_pattern().to_string(),
            }
        } else {
            return Err(ProjectError::MissingEmbeddingConfig {
                resolver: resolver.to_string(),
            });
        };
        providers.insert(resolver.to_string(), strategy);
    }
    Ok(ExtractionRegistry(providers))
}

fn strategy_for(resolver: &str, config: &EmbeddingConfig) -> Result<ExtractionStrategy> {
    match config.strategy {
        EmbeddingStrategy::Regex => {
            let pattern =
                config
                    .pattern
                    .clone()
                    .ok_or_else(|| ProjectError::MissingEmbeddingPattern {
                        resolver: resolver.to_string(),
                    })?;
            compile_embedding_pattern(&pattern).map_err(|message| {
                ProjectError::InvalidEmbeddingPattern {
                    resolver: resolver.to_string(),
                    message,
                }
            })?;
            Ok(ExtractionStrategy::Regex { pattern })
        }
    }
}
