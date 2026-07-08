//! Project loading: `dsql.toml` discovery and parsing, schema catalog
//! loading, document discovery, and bowl assembly for adapters.

mod config;
mod documents;
mod metadata;
mod open;

pub use config::{Config, Project, ProjectError, find_root};
pub use documents::{ProjectDocument, load_project_documents};
pub use metadata::load_metadata_dir;
pub use open::open_project_bowl;
