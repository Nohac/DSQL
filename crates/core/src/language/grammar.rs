use crate::{
    language::{atom::AtomDescriptor, atoms::directive::DirectiveAtom},
    syntax::grammar::parser::Rule,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleClassification {
    Owned(AtomDescriptor),
    Legacy,
    Internal,
}

/// Classifies every generated Lelwel rule as atom-owned or intentionally internal.
///
/// This match must stay exhaustive and must not use a wildcard arm. Adding a
/// grammar rule to `dsql.llw` then fails compilation until that new rule gets
/// an ownership decision here.
pub const fn classify_rule(rule: Rule) -> RuleClassification {
    match rule {
        Rule::BinaryExpr => RuleClassification::Legacy,
        Rule::BinaryOperator => RuleClassification::Legacy,
        Rule::Clause => RuleClassification::Legacy,
        Rule::ClauseList => RuleClassification::Internal,
        Rule::ComparisonOperator => RuleClassification::Legacy,
        Rule::Definition => RuleClassification::Internal,
        Rule::Directive => RuleClassification::Owned(DirectiveAtom::DESCRIPTOR),
        Rule::Document => RuleClassification::Internal,
        Rule::Error => RuleClassification::Internal,
        Rule::Expr => RuleClassification::Legacy,
        Rule::FieldSelection => RuleClassification::Legacy,
        Rule::FieldSelectionTail => RuleClassification::Internal,
        Rule::FieldSuffix => RuleClassification::Internal,
        Rule::FragmentDef => RuleClassification::Legacy,
        Rule::FragmentSpread => RuleClassification::Legacy,
        Rule::LimitClause => RuleClassification::Legacy,
        Rule::Literal => RuleClassification::Legacy,
        Rule::OffsetClause => RuleClassification::Legacy,
        Rule::OperatorVariable => RuleClassification::Legacy,
        Rule::OrderByClause => RuleClassification::Legacy,
        Rule::OrderItem => RuleClassification::Legacy,
        Rule::QualifiedName => RuleClassification::Legacy,
        Rule::QueryDef => RuleClassification::Legacy,
        Rule::RelationRef => RuleClassification::Internal,
        Rule::ScopedPath => RuleClassification::Legacy,
        Rule::ScopedPathSegment => RuleClassification::Legacy,
        Rule::Selection => RuleClassification::Internal,
        Rule::SelectionSet => RuleClassification::Internal,
        Rule::SortDirection => RuleClassification::Legacy,
        Rule::ValueVariable => RuleClassification::Legacy,
        Rule::WhereClause => RuleClassification::Legacy,
    }
}

const _: RuleClassification = classify_rule(Rule::Directive);
