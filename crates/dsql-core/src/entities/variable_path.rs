//! Variable path construction.
//!
//! Sigil mapping: `$` (build-time) variables are *structured* — they nest
//! under `input.<selection path>.clause...`; `%` top-level parameters are
//! *top-level* — they surface as `params.<name>`.

use crate::entities::expression::{BinaryOp, Sigil};
use crate::entities::variable::{InputDefault, SpreadInputValue, VariableRole};

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::AsRefStr)]
#[strum(serialize_all = "snake_case")]
pub enum InputPathSegment {
    Input,
    Params,
    Body,
    Aggregate,
    Clause,
    Where,
    OrderBy,
    Limit,
    Offset,
    Filter,
    Value,
    Op,
    Direction,
}

#[derive(Clone)]
pub(crate) struct SelectionPath {
    pub(crate) parts: Vec<String>,
    pub(crate) mode: SelectionPathMode,
}

impl SelectionPath {
    pub(crate) fn body(parts: Vec<String>) -> Self {
        Self {
            parts,
            mode: SelectionPathMode::Body,
        }
    }

    pub(crate) fn fragment_root() -> Self {
        Self {
            parts: Vec::new(),
            mode: SelectionPathMode::FragmentRoot,
        }
    }

    pub(crate) fn relation_child_path(&self, output_name: impl Into<String>) -> Vec<String> {
        let mut child_path = self.parts.clone();
        if matches!(self.mode, SelectionPathMode::Body) {
            child_path.push(InputPathSegment::Body.as_ref().to_string());
        }
        child_path.push(output_name.into());
        child_path
    }
}

#[derive(Clone, Copy)]
pub(crate) enum SelectionPathMode {
    Body,
    FragmentRoot,
}

#[derive(Clone)]
pub(crate) struct VariablePathScope {
    pub(crate) structured_prefix: Vec<String>,
    pub(crate) top_level_prefix: Vec<String>,
    frames: Vec<std::collections::BTreeMap<String, SpreadInputValue>>,
}

impl VariablePathScope {
    pub(crate) fn operation() -> Self {
        Self {
            structured_prefix: vec![InputPathSegment::Input.as_ref().to_string()],
            top_level_prefix: vec![InputPathSegment::Params.as_ref().to_string()],
            frames: Vec::new(),
        }
    }

    pub(crate) fn fragment() -> Self {
        Self::operation()
    }

    /// Enters a fragment using the semantic path/default mapping for its spread.
    pub(crate) fn for_spread_map(
        &self,
        values: std::collections::BTreeMap<String, SpreadInputValue>,
    ) -> Self {
        let mut frames = self.frames.clone();
        frames.push(values);
        Self {
            structured_prefix: vec![InputPathSegment::Input.as_ref().to_string()],
            top_level_prefix: vec![InputPathSegment::Params.as_ref().to_string()],
            frames,
        }
    }
}

/// The stable generated key for an anonymous predicate value paired with a
/// dynamic comparison operator.
pub(crate) fn predicate_anonymous_key(operator: &BinaryOp) -> Option<&'static str> {
    matches!(operator, BinaryOp::Variable(_)).then_some(InputPathSegment::Value.as_ref())
}

/// The planned value of a variable after all enclosing spread bindings apply.
pub(crate) enum VariableValue {
    Public(String),
    Default(InputDefault),
}

pub(crate) fn fragment_envelope_path(
    current_path: &SelectionPath,
    fragment_name: &str,
) -> Vec<String> {
    let mut parts = current_path.parts.clone();
    if matches!(current_path.mode, SelectionPathMode::Body) {
        parts.push(InputPathSegment::Body.as_ref().to_string());
    }
    parts.push(fragment_name.to_string());
    parts
}

impl VariableRole {
    fn path_clause_segment(self) -> InputPathSegment {
        match self {
            Self::WhereValue | Self::DynamicPredicate | Self::ComparisonOperator => {
                InputPathSegment::Where
            }
            Self::SortDirection | Self::DynamicOrder => InputPathSegment::OrderBy,
            Self::Limit => InputPathSegment::Limit,
            Self::Offset => InputPathSegment::Offset,
            Self::FilterAssignment => InputPathSegment::Filter,
        }
    }

    fn path_includes_inferred_path(self) -> bool {
        matches!(
            self,
            Self::WhereValue
                | Self::DynamicPredicate
                | Self::ComparisonOperator
                | Self::SortDirection
                | Self::DynamicOrder
                | Self::FilterAssignment
        )
    }
}

#[derive(Clone, Copy)]
pub(crate) struct VariablePathContext<'a> {
    pub(crate) role: VariableRole,
    pub(crate) inferred_path: &'a [String],
    pub(crate) anonymous_key: Option<&'a str>,
}

pub(crate) fn variable_path(
    selection_path: &[String],
    context: VariablePathContext<'_>,
    variable_scope: &VariablePathScope,
    scope: Sigil,
    name: Option<&str>,
) -> String {
    match variable_value(selection_path, context, variable_scope, scope, name) {
        VariableValue::Public(path) => path,
        // Callers that can lower constants use [`variable_value`]. Keeping
        // the native path here makes unsupported constant roles diagnose as
        // undeclared inputs instead of silently binding the wrong value.
        VariableValue::Default(_) => {
            native_variable_path(selection_path, context, variable_scope, scope, name)
        }
    }
}

pub(crate) fn variable_value(
    selection_path: &[String],
    context: VariablePathContext<'_>,
    variable_scope: &VariablePathScope,
    scope: Sigil,
    name: Option<&str>,
) -> VariableValue {
    let path = native_variable_path(selection_path, context, variable_scope, scope, name);
    variable_scope
        .frames
        .iter()
        .rev()
        .fold(VariableValue::Public(path), |value, frame| match value {
            VariableValue::Public(path) => match frame.get(&path) {
                Some(SpreadInputValue::Public(path)) => VariableValue::Public(path.clone()),
                Some(SpreadInputValue::Default(default)) => VariableValue::Default(default.clone()),
                None => VariableValue::Public(path),
            },
            VariableValue::Default(default) => VariableValue::Default(default),
        })
}

fn native_variable_path(
    selection_path: &[String],
    context: VariablePathContext<'_>,
    variable_scope: &VariablePathScope,
    scope: Sigil,
    name: Option<&str>,
) -> String {
    let key = name.map_or_else(
        || {
            if let Some(key) = context.anonymous_key {
                key.to_string()
            } else if matches!(context.role, VariableRole::ComparisonOperator) {
                InputPathSegment::Op.as_ref().to_string()
            } else {
                context.inferred_path.last().cloned().unwrap_or_default()
            }
        },
        ToString::to_string,
    );
    match scope {
        Sigil::Build => structured_variable_path(
            selection_path,
            &context,
            variable_scope,
            name.is_some()
                || context.anonymous_key.is_some()
                || matches!(context.role, VariableRole::ComparisonOperator),
            key,
        ),
        Sigil::Query => join_prefixed_key(&variable_scope.top_level_prefix, &key),
        Sigil::Context => join_prefixed_key(&["context".to_string()], &key),
    }
}

pub(crate) fn join_prefixed_key(prefix: &[String], key: &str) -> String {
    let mut parts = prefix.to_vec();
    parts.push(key.to_string());
    parts.join(".")
}

pub fn is_params_path(path: &str) -> bool {
    matches!(
        path.split_once('.'),
        Some((prefix, _)) if prefix == InputPathSegment::Params.as_ref()
    )
}

pub fn is_input_path(path: &str) -> bool {
    matches!(
        path.split_once('.'),
        Some((prefix, _)) if prefix == InputPathSegment::Input.as_ref()
    )
}

fn structured_variable_path(
    selection_path: &[String],
    context: &VariablePathContext<'_>,
    variable_scope: &VariablePathScope,
    include_key: bool,
    key: String,
) -> String {
    let mut parts = variable_scope.structured_prefix.clone();
    parts.extend(selection_path.iter().cloned());
    parts.push(InputPathSegment::Clause.as_ref().to_string());
    parts.push(context.role.path_clause_segment().as_ref().to_string());
    if context.role.path_includes_inferred_path() {
        parts.extend(context.inferred_path.iter().cloned());
    }
    if include_key {
        parts.push(key);
    }
    parts.join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_limit_path_matches_existing_shape() {
        let scope = VariablePathScope::operation();
        let selection = vec!["title".to_string()];

        assert_eq!(
            variable_path(
                &selection,
                VariablePathContext {
                    role: VariableRole::Limit,
                    inferred_path: &[InputPathSegment::Limit.as_ref().to_string()],
                    anonymous_key: None,
                },
                &scope,
                Sigil::Build,
                None,
            ),
            "input.title.clause.limit",
        );
    }
}
