//! Project loading: `dsql.toml` discovery and parsing, schema catalog
//! loading, document discovery, and bowl assembly for adapters.

mod config;
mod documents;
mod embedding;
mod metadata;
mod open;

pub use config::{Config, GenerateConfig, Project, ProjectError, ScopeConfig, TypescriptGenerateConfig, find_root};
pub use documents::{ProjectDocument, load_project_documents};
pub use metadata::load_metadata_dir;
pub use open::{open_project_bowl, populate_project_bowl};
