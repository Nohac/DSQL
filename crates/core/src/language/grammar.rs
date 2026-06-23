use crate::{
    language::{atom::AtomDescriptor, atoms::directive::DirectiveAtom},
    syntax::grammar::parser::Rule,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleClassification {
    Owned(AtomDescriptor),
    Internal(&'static str),
}

/// Classifies every generated Lelwel rule as atom-owned or intentionally internal.
///
/// This match must stay exhaustive and must not use a wildcard arm. Adding a
/// grammar rule to `dsql.llw` then fails compilation until that new rule gets
/// an ownership decision here.
pub const fn classify_rule(rule: Rule) -> RuleClassification {
    match rule {
        Rule::BinaryExpr => {
            RuleClassification::Internal("expression structure; not migrated to an atom yet")
        }
        Rule::BinaryOperator => {
            RuleClassification::Internal("operator structure; not migrated to an atom yet")
        }
        Rule::Clause => {
            RuleClassification::Internal("clause grouping; not migrated to an atom yet")
        }
        Rule::ClauseList => {
            RuleClassification::Internal("groups clauses attached to a field selection")
        }
        Rule::ComparisonOperator => RuleClassification::Internal("operator token grouping"),
        Rule::Definition => RuleClassification::Internal("groups query and fragment definitions"),
        Rule::Directive => RuleClassification::Owned(DirectiveAtom::DESCRIPTOR),
        Rule::Document => RuleClassification::Internal("top-level container"),
        Rule::Error => RuleClassification::Internal("parser recovery rule"),
        Rule::Expr => {
            RuleClassification::Internal("expression wrapper; not migrated to an atom yet")
        }
        Rule::FieldSelection => {
            RuleClassification::Internal("source construct pending a field selection atom")
        }
        Rule::FieldSelectionTail => {
            RuleClassification::Internal("groups aliases, field suffixes, and selections")
        }
        Rule::FieldSuffix => {
            RuleClassification::Internal("groups clauses, directives, and nested selections")
        }
        Rule::FragmentDef => {
            RuleClassification::Internal("source construct pending a fragment definition atom")
        }
        Rule::FragmentSpread => {
            RuleClassification::Internal("source construct pending a fragment spread atom")
        }
        Rule::LimitClause => {
            RuleClassification::Internal("clause source construct pending a clause atom")
        }
        Rule::Literal => {
            RuleClassification::Internal("literal expression wrapper; not migrated to an atom yet")
        }
        Rule::OffsetClause => {
            RuleClassification::Internal("clause source construct pending a clause atom")
        }
        Rule::OperatorVariable => {
            RuleClassification::Internal("operator variable expression wrapper")
        }
        Rule::OrderByClause => {
            RuleClassification::Internal("clause source construct pending a clause atom")
        }
        Rule::OrderItem => RuleClassification::Internal("order-by item structure"),
        Rule::QualifiedName => {
            RuleClassification::Internal("name structure shared by multiple atoms")
        }
        Rule::QueryDef => {
            RuleClassification::Internal("source construct pending a query definition atom")
        }
        Rule::RelationRef => RuleClassification::Internal(
            "relation reference structure shared by selections and paths",
        ),
        Rule::ScopedPath => RuleClassification::Internal("path expression structure"),
        Rule::ScopedPathSegment => RuleClassification::Internal("path segment structure"),
        Rule::Selection => {
            RuleClassification::Internal("selection wrapper pending dedicated selection atoms")
        }
        Rule::SelectionSet => RuleClassification::Internal("groups nested selections"),
        Rule::SortDirection => RuleClassification::Internal("sort direction wrapper"),
        Rule::ValueVariable => RuleClassification::Internal("value variable expression wrapper"),
        Rule::WhereClause => {
            RuleClassification::Internal("clause source construct pending a clause atom")
        }
    }
}

const _: RuleClassification = classify_rule(Rule::Directive);
