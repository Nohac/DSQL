//! Normalized, in-memory inputs for one language workspace.
//!
//! Native project loading and browser bindings both translate their own
//! storage/configuration model into this value. The compiler only sees facts
//! installed in the bowl; it never needs to know where they came from.

use bowl::{Bowl, Singleton};

use crate::catalog::{Catalog, insert_catalog};
use crate::embedding::ExtractionRegistry;
use crate::lint::LintConfig;
use crate::source::{
    ResolutionScope, ScopeDocuments, ScopeImports, SourceKind, insert_source_scoped,
};

/// One physical source supplied to the language engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanguageDocument {
    /// Stable logical identity used in diagnostics and source maps.
    pub path: String,
    /// Complete source contents.
    pub text: String,
    /// Resolution scope that owns the document.
    pub scope: ResolutionScope,
    /// Whether the source is a standalone document or an embedding host.
    pub kind: SourceKind,
}

/// Complete storage-independent inputs needed to populate a language bowl.
pub struct LanguageInputs {
    /// Introspected database schema used by resolution and planning.
    pub catalog: Catalog,
    /// Physical source documents and embedding hosts.
    pub documents: Vec<LanguageDocument>,
    /// Resolution-scope import graph.
    pub scope_imports: ScopeImports,
    /// Configured path ownership used when classifying newly introduced files.
    pub scope_documents: ScopeDocuments,
    /// Named embedded-document extractors.
    pub extraction_registry: ExtractionRegistry,
    /// Optional lint configuration; absence keeps lints unarmed.
    pub lint: Option<LintConfig>,
}

/// Installs normalized inputs into an already registered, fresh language bowl.
pub async fn populate_language_bowl(bowl: &Bowl, inputs: LanguageInputs) {
    insert_catalog(bowl, inputs.catalog).await;
    bowl.insert((Singleton::<ScopeImports>::new(), inputs.scope_imports))
        .await;
    bowl.insert((Singleton::<ScopeDocuments>::new(), inputs.scope_documents))
        .await;
    bowl.insert((
        Singleton::<ExtractionRegistry>::new(),
        inputs.extraction_registry,
    ))
    .await;
    if let Some(lint) = inputs.lint {
        bowl.insert((Singleton::<LintConfig>::new(), lint)).await;
    }

    for document in inputs.documents {
        insert_source_scoped(
            bowl,
            document.path,
            &document.text,
            document.scope,
            document.kind,
        )
        .await;
    }
}
