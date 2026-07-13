//! Project loading: `dsql.toml` discovery and parsing, schema catalog
//! loading, document discovery, and bowl assembly for adapters.

mod config;
mod documents;
mod embedding;
mod metadata;
mod open;

pub use config::{
    Config, GenerateConfig, LintSectionConfig, LintSeverity, Project, ProjectError, ScopeConfig,
    TypescriptGenerateConfig, find_root, init_project, validate_reserved_root,
};
pub use documents::{ProjectDocument, load_project_documents, load_project_documents_excluding};
pub use metadata::{load_metadata_dir, store_metadata_dir};
pub use open::{
    open_analysis_bowl, open_project_bowl, populate_project_bowl, populate_project_bowl_excluding,
};
