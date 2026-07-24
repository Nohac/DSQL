//! Compiler-owned project contract shared by daemon handoffs and generated
//! TypeScript wiring.

use std::collections::BTreeMap;

use facet::Facet;

use dsql_core::source::ScopeImports;

use crate::{GenerateError, snapshot::sha256_hex};

/// Canonical fingerprint representation used by Rust and TypeScript clients.
#[derive(Clone, Debug, Facet, PartialEq, Eq)]
pub struct ProjectContractFingerprint {
    pub algorithm: String,
    pub value: String,
}

/// One configured resolution scope in the generated project contract.
#[derive(Clone, Debug, Facet, PartialEq, Eq)]
pub struct ProjectContractScope {
    pub name: String,
    pub imports: Vec<String>,
    #[facet(rename = "generationTarget")]
    pub generation_target: bool,
}

/// The compiler-owned scope and generator-type contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectContract {
    pub fingerprint: ProjectContractFingerprint,
    pub scopes: Vec<ProjectContractScope>,
}

#[derive(Facet)]
struct CanonicalProjectContract {
    scopes: Vec<ProjectContractScope>,
    /// External generator-visible directives are not implemented yet.
    /// Keeping the honest empty projection in the canonical payload makes
    /// the future registry an additive input to this same fingerprint.
    directives: BTreeMap<String, String>,
}

impl ProjectContract {
    /// Constructs the one canonical project contract from the complete
    /// configured scope graph.
    pub fn from_imports(imports: &ScopeImports) -> Result<Self, GenerateError> {
        let scopes = imports
            .0
            .iter()
            .map(|(name, direct_imports)| {
                let mut direct_imports = direct_imports.clone();
                direct_imports.sort();
                direct_imports.dedup();
                ProjectContractScope {
                    name: name.clone(),
                    imports: direct_imports,
                    generation_target: imports.is_generation_target(name),
                }
            })
            .collect();
        let canonical = CanonicalProjectContract {
            scopes,
            directives: BTreeMap::new(),
        };
        let serialized =
            facet_json::to_string(&canonical).map_err(|error| GenerateError::Serialize {
                name: "project contract".to_string(),
                message: error.to_string(),
            })?;
        Ok(Self {
            fingerprint: ProjectContractFingerprint {
                algorithm: "sha256".to_string(),
                value: sha256_hex(serialized.as_bytes()),
            },
            scopes: canonical.scopes,
        })
    }

    /// Renders the reproducible `dsql/project.generated.ts` source contract.
    pub fn typescript_source(&self) -> Result<String, GenerateError> {
        let fingerprint =
            facet_json::to_string(&self.fingerprint).map_err(|error| GenerateError::Serialize {
                name: "project contract fingerprint".to_string(),
                message: error.to_string(),
            })?;
        let targets: Vec<&str> = self
            .scopes
            .iter()
            .filter(|scope| scope.generation_target)
            .map(|scope| scope.name.as_str())
            .collect();
        let targets =
            facet_json::to_string(&targets).map_err(|error| GenerateError::Serialize {
                name: "project contract targets".to_string(),
                message: error.to_string(),
            })?;
        let mut scope_entries = Vec::new();
        for scope in &self.scopes {
            let name =
                facet_json::to_string(&scope.name).map_err(|error| GenerateError::Serialize {
                    name: "project contract scope".to_string(),
                    message: error.to_string(),
                })?;
            let imports = facet_json::to_string(&scope.imports).map_err(|error| {
                GenerateError::Serialize {
                    name: format!("project contract scope `{}`", scope.name),
                    message: error.to_string(),
                }
            })?;
            scope_entries.push(format!("    [{name}]: {{ imports: {imports} }}"));
        }
        Ok(format!(
            "import {{ defineDsqlProject }} from \"@dsql/typescript/renderer\";\n\n\
             export const project = defineDsqlProject({{\n\
             \x20 contractHash: {fingerprint},\n\
             \x20 scopes: {{\n{}\n\
             \x20 }},\n\
             \x20 targets: {targets},\n\
             \x20 directives: {{}},\n\
             }});\n",
            scope_entries.join(",\n"),
        ))
    }
}
