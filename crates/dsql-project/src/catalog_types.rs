//! Strict authored mappings from provider types to public scalar semantics.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use dsql_core::catalog::{
    Catalog, CatalogType, CatalogTypeShape, LiteralKind, TypeCapabilities, TypeId, TypeKey,
    WireEncoding,
};
use dsql_core::entities::expression::ComparisonOp;
use regex::Regex;

use crate::config::{
    CatalogTypeConfig, CatalogTypeLiteral, CatalogTypeOperator, CatalogTypeWire, ProjectError,
    Result,
};

struct Mapping<'a> {
    index: usize,
    config: &'a CatalogTypeConfig,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResolutionState {
    Pending,
    Resolving,
    Resolved,
}

pub(crate) fn apply_catalog_types(
    mut catalog: Catalog,
    declarations: &[CatalogTypeConfig],
    config_path: &Path,
) -> Result<Catalog> {
    let mut mappings = HashMap::<TypeId, Mapping<'_>>::new();
    let mut names = HashMap::<&str, usize>::new();

    for (index, declaration) in declarations.iter().enumerate() {
        validate_name(declaration, index, config_path)?;
        validate_pattern(declaration, index, config_path)?;
        if let Some(first) = names.insert(&declaration.name, index) {
            return mapping_error(
                config_path,
                index,
                "name",
                format!(
                    "nominal name `{}` is also declared by catalog.types[{first}]",
                    declaration.name
                ),
            );
        }

        let key = parse_provider_key(&declaration.pg).ok_or_else(|| ProjectError::CatalogType {
            path: config_path.to_path_buf(),
            item_path: format!("catalog.types[{index}].pg"),
            message: "must be exactly `schema.type`".to_string(),
        })?;
        let data_type = catalog
            .type_by_key(&key)
            .ok_or_else(|| ProjectError::CatalogType {
                path: config_path.to_path_buf(),
                item_path: format!("catalog.types[{index}].pg"),
                message: format!("provider type `{}` was not found", declaration.pg),
            })?;
        if matches!(data_type.shape, CatalogTypeShape::Array { .. }) {
            return mapping_error(
                config_path,
                index,
                "pg",
                "array types inherit their element mapping and cannot be declared directly",
            );
        }
        if let Some(first) = mappings.insert(
            data_type.id,
            Mapping {
                index,
                config: declaration,
            },
        ) {
            return mapping_error(
                config_path,
                index,
                "pg",
                format!(
                    "provider type `{}` is already declared by catalog.types[{}]",
                    declaration.pg, first.index
                ),
            );
        }
    }

    let original = catalog.types.clone();
    let mut states = vec![ResolutionState::Pending; original.len()];
    let mut resolved = vec![None; original.len()];
    for index in 0..original.len() {
        resolve_capabilities(
            index,
            &original,
            &mappings,
            &mut states,
            &mut resolved,
            config_path,
        )?;
    }
    for (index, capabilities) in resolved.into_iter().enumerate() {
        let Some(capabilities) = capabilities else {
            return Err(ProjectError::CatalogType {
                path: config_path.to_path_buf(),
                item_path: "catalog.types".to_string(),
                message: format!("catalog type at index {index} did not resolve"),
            });
        };
        if let Some(data_type) = catalog.types.get_mut(index) {
            data_type.capabilities = capabilities;
        }
    }
    Ok(catalog)
}

fn resolve_capabilities(
    index: usize,
    types: &[CatalogType],
    mappings: &HashMap<TypeId, Mapping<'_>>,
    states: &mut [ResolutionState],
    resolved: &mut [Option<TypeCapabilities>],
    config_path: &Path,
) -> Result<()> {
    match states.get(index) {
        Some(ResolutionState::Resolved) => return Ok(()),
        Some(ResolutionState::Resolving) => {
            return Err(ProjectError::CatalogType {
                path: config_path.to_path_buf(),
                item_path: "catalog.types".to_string(),
                message: "catalog type structure is cyclic".to_string(),
            });
        }
        Some(ResolutionState::Pending) => {}
        None => {
            return Err(ProjectError::CatalogType {
                path: config_path.to_path_buf(),
                item_path: "catalog.types".to_string(),
                message: format!("catalog type index {index} is invalid"),
            });
        }
    }
    states[index] = ResolutionState::Resolving;
    let Some(data_type) = types.get(index) else {
        return Err(ProjectError::CatalogType {
            path: config_path.to_path_buf(),
            item_path: "catalog.types".to_string(),
            message: format!("catalog type index {index} is missing"),
        });
    };
    let mut capabilities = data_type.capabilities.clone();
    if let CatalogTypeShape::Domain { base } = data_type.shape {
        resolve_capabilities(base.0, types, mappings, states, resolved, config_path)?;
        let Some(base_capabilities) = resolved.get(base.0).and_then(Option::as_ref) else {
            return Err(ProjectError::CatalogType {
                path: config_path.to_path_buf(),
                item_path: "catalog.types".to_string(),
                message: format!(
                    "base type for `{}.{}` did not resolve",
                    data_type.key.schema, data_type.key.name
                ),
            });
        };
        capabilities.inherit_mapped_domain(base_capabilities);
    }
    if let Some(mapping) = mappings.get(&data_type.id) {
        apply_mapping(&mut capabilities, mapping, config_path)?;
    }
    resolved[index] = Some(capabilities);
    states[index] = ResolutionState::Resolved;
    Ok(())
}

fn apply_mapping(
    capabilities: &mut TypeCapabilities,
    mapping: &Mapping<'_>,
    config_path: &Path,
) -> Result<()> {
    let declaration = mapping.config;
    if !declaration.wire.matches(capabilities.wire) {
        return mapping_error(
            config_path,
            mapping.index,
            "wire",
            format!(
                "wire `{}` is incompatible with provider encoding `{}`",
                declaration.wire.as_str(),
                capabilities.wire.as_str()
            ),
        );
    }
    if !declaration.literal.matches(declaration.wire) {
        return mapping_error(
            config_path,
            mapping.index,
            "literal",
            format!(
                "literal `{}` is incompatible with wire `{}`",
                declaration.literal.as_str(),
                declaration.wire.as_str()
            ),
        );
    }
    if declaration.pattern.is_some()
        && (declaration.wire != CatalogTypeWire::Text
            || declaration.literal != CatalogTypeLiteral::String)
    {
        return mapping_error(
            config_path,
            mapping.index,
            "pattern",
            "patterns require `wire = \"text\"` and `literal = \"string\"`",
        );
    }
    if declaration.orderable == Some(true) {
        return mapping_error(
            config_path,
            mapping.index,
            "orderable",
            "may only be omitted or set to false",
        );
    }

    let operators = declaration
        .operators
        .as_ref()
        .map(|operators| {
            let mut requested = Vec::with_capacity(operators.len());
            let mut seen = HashSet::new();
            for configured in operators {
                let operator = configured.comparison();
                if !seen.insert(operator) {
                    return mapping_error(
                        config_path,
                        mapping.index,
                        "operators",
                        format!("operator `{}` is duplicated", configured.as_str()),
                    );
                }
                if !capabilities.supports(operator) {
                    return mapping_error(
                        config_path,
                        mapping.index,
                        "operators",
                        format!(
                            "provider type does not support operator `{}`",
                            configured.as_str()
                        ),
                    );
                }
                requested.push(operator);
            }
            Ok(requested)
        })
        .transpose()?;

    capabilities.apply_mapping(
        declaration.name.clone(),
        declaration.literal.literal_kind(),
        declaration.pattern.clone(),
        operators,
        declaration.orderable,
    );
    Ok(())
}

fn validate_name(declaration: &CatalogTypeConfig, index: usize, config_path: &Path) -> Result<()> {
    let mut characters = declaration.name.chars();
    let valid = characters
        .next()
        .is_some_and(|first| first.is_ascii_uppercase())
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_');
    if !valid {
        return mapping_error(
            config_path,
            index,
            "name",
            "must match `[A-Z][A-Za-z0-9_]*`",
        );
    }
    let collides_with_builtin = dsql_core::catalog::DataType::ALL
        .into_iter()
        .flat_map(|data_type| Catalog::builtin_capabilities(data_type).aliases)
        .any(|alias| alias.eq_ignore_ascii_case(&declaration.name));
    if collides_with_builtin {
        return mapping_error(
            config_path,
            index,
            "name",
            format!(
                "nominal name `{}` collides with a compiler-owned type",
                declaration.name
            ),
        );
    }
    Ok(())
}

fn validate_pattern(
    declaration: &CatalogTypeConfig,
    index: usize,
    config_path: &Path,
) -> Result<()> {
    let Some(pattern) = declaration.pattern.as_deref() else {
        return Ok(());
    };
    validate_portable_pattern(pattern).map_err(|message| ProjectError::CatalogType {
        path: config_path.to_path_buf(),
        item_path: format!("catalog.types[{index}].pattern"),
        message,
    })
}

fn validate_portable_pattern(pattern: &str) -> std::result::Result<(), String> {
    let mut characters = pattern.char_indices().peekable();
    let mut in_class = false;
    while let Some((offset, character)) = characters.next() {
        if character == '\\' {
            let Some((_, escaped)) = characters.next() else {
                return Err("ends with an incomplete escape".to_string());
            };
            match escaped {
                'd' | 'D' | 'w' | 'W' | 's' | 'S' | 'p' | 'P' | 'b' | 'B' | 'A' | 'z' => {
                    return Err(format!(
                        "escape `\\{escaped}` is not in the portable regex subset"
                    ));
                }
                'x' => consume_hex(&mut characters, 2, "\\xNN")?,
                'u' => consume_hex(&mut characters, 4, "\\uNNNN")?,
                'n' | 'r' | 't' => {}
                '-' if !in_class => {
                    return Err(
                        "escape `\\-` is only portable inside a character class".to_string()
                    );
                }
                '^' | '$' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}'
                | '|' | '/' | '-' => {}
                _ => {
                    return Err(format!(
                        "escape `\\{escaped}` is not in the portable regex subset"
                    ));
                }
            }
            continue;
        }
        if in_class {
            match character {
                ']' => in_class = false,
                '[' => {
                    return Err(
                        "nested character classes are not in the portable regex subset".to_string(),
                    );
                }
                '&' | '-' | '~'
                    if characters
                        .peek()
                        .is_some_and(|(_, next)| *next == character) =>
                {
                    return Err(
                        "character-class set operations are not in the portable regex subset"
                            .to_string(),
                    );
                }
                _ => {}
            }
            continue;
        }
        match character {
            '[' => in_class = true,
            ']' => return Err("contains an unmatched `]`".to_string()),
            '.' => {
                return Err(
                    "wildcard `.` is not in the portable regex subset; use an explicit negated character class"
                        .to_string(),
                );
            }
            '{' => consume_quantifier(&mut characters)?,
            '}' => return Err("contains an unmatched `}`".to_string()),
            '(' if pattern[offset..].starts_with("(?") && !pattern[offset..].starts_with("(?:") => {
                return Err(
                    "only non-capturing `(?:...)` groups from the `(?...)` family are portable"
                        .to_string(),
                );
            }
            _ => {}
        }
    }
    Regex::new(pattern)
        .map(|_| ())
        .map_err(|error| format!("is not a valid portable regex: {error}"))
}

fn consume_quantifier(
    characters: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
) -> std::result::Result<(), String> {
    let mut minimum_digits = 0;
    while characters
        .peek()
        .is_some_and(|(_, character)| character.is_ascii_digit())
    {
        characters.next();
        minimum_digits += 1;
    }
    if minimum_digits == 0 {
        return Err(
            "repeat quantifiers must use `{n}`, `{n,}`, or `{n,m}` with ASCII digits".to_string(),
        );
    }
    match characters.next() {
        Some((_, '}')) => Ok(()),
        Some((_, ',')) => {
            while characters
                .peek()
                .is_some_and(|(_, character)| character.is_ascii_digit())
            {
                characters.next();
            }
            match characters.next() {
                Some((_, '}')) => Ok(()),
                _ => Err(
                    "repeat quantifiers must use `{n}`, `{n,}`, or `{n,m}` with ASCII digits"
                        .to_string(),
                ),
            }
        }
        _ => Err(
            "repeat quantifiers must use `{n}`, `{n,}`, or `{n,m}` with ASCII digits".to_string(),
        ),
    }
}

fn consume_hex(
    characters: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    count: usize,
    expected: &str,
) -> std::result::Result<(), String> {
    for _ in 0..count {
        let Some((_, character)) = characters.next() else {
            return Err(format!("escape must use exactly `{expected}`"));
        };
        if !character.is_ascii_hexdigit() {
            return Err(format!("escape must use exactly `{expected}`"));
        }
    }
    Ok(())
}

fn parse_provider_key(raw: &str) -> Option<TypeKey> {
    let mut parts = raw.split('.');
    let schema = parts.next()?;
    let name = parts.next()?;
    (!schema.is_empty() && !name.is_empty() && parts.next().is_none())
        .then(|| TypeKey::new(schema, name))
}

fn mapping_error<T>(
    path: &Path,
    index: usize,
    field: &str,
    message: impl Into<String>,
) -> Result<T> {
    Err(ProjectError::CatalogType {
        path: PathBuf::from(path),
        item_path: format!("catalog.types[{index}].{field}"),
        message: message.into(),
    })
}

impl CatalogTypeWire {
    fn matches(self, wire: WireEncoding) -> bool {
        matches!(
            (self, wire),
            (Self::Uuid, WireEncoding::Uuid)
                | (Self::Text, WireEncoding::Text | WireEncoding::TextCast)
                | (Self::Timestamptz, WireEncoding::Timestamptz)
                | (Self::Integer, WireEncoding::Integer)
                | (Self::BigInteger, WireEncoding::BigInteger)
                | (Self::Numeric, WireEncoding::Numeric)
                | (Self::Float, WireEncoding::Float)
                | (Self::Boolean, WireEncoding::Boolean)
                | (Self::Json, WireEncoding::Json)
        )
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Uuid => "uuid",
            Self::Text => "text",
            Self::Timestamptz => "timestamptz",
            Self::Integer => "integer",
            Self::BigInteger => "big_integer",
            Self::Numeric => "numeric",
            Self::Float => "float",
            Self::Boolean => "boolean",
            Self::Json => "json",
        }
    }
}

impl CatalogTypeLiteral {
    fn literal_kind(self) -> LiteralKind {
        match self {
            Self::String => LiteralKind::String,
            Self::Number => LiteralKind::Number,
            Self::Boolean => LiteralKind::Boolean,
        }
    }

    fn matches(self, wire: CatalogTypeWire) -> bool {
        matches!(
            (self, wire),
            (
                Self::String,
                CatalogTypeWire::Uuid
                    | CatalogTypeWire::Text
                    | CatalogTypeWire::Timestamptz
                    | CatalogTypeWire::BigInteger
                    | CatalogTypeWire::Json
            ) | (
                Self::Number,
                CatalogTypeWire::Integer | CatalogTypeWire::Numeric | CatalogTypeWire::Float
            ) | (Self::Boolean, CatalogTypeWire::Boolean)
        )
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
        }
    }
}

impl CatalogTypeOperator {
    fn as_str(self) -> &'static str {
        match self {
            Self::Eq => "eq",
            Self::Ne => "ne",
            Self::Gt => "gt",
            Self::Ge => "ge",
            Self::Lt => "lt",
            Self::Le => "le",
            Self::Like => "like",
        }
    }

    fn comparison(self) -> ComparisonOp {
        match self {
            Self::Eq => ComparisonOp::Eq,
            Self::Ne => ComparisonOp::Ne,
            Self::Gt => ComparisonOp::Gt,
            Self::Ge => ComparisonOp::Ge,
            Self::Lt => ComparisonOp::Lt,
            Self::Le => ComparisonOp::Le,
            Self::Like => ComparisonOp::Like,
        }
    }
}
