//! Leaf-construct lowering: clauses carry typed expression trees, directives
//! carry their names and arguments, and every variable occurrence becomes a
//! fact anchored to the clause or directive containing it.

use bowl::{Bowl, Entity, Query};
use dsql_core::entities::clause::{ClauseFact, OrderDirection};
use dsql_core::entities::directive::DirectiveFact;
use dsql_core::entities::field_selection::FieldSel;
use dsql_core::entities::variable::VariableUse;
use dsql_core::facts::{ChildOf, NodeKey};
use dsql_core::source::insert_source;

use crate::fixture;

async fn language_bowl() -> Bowl {
    dsql_core::language_bowl().await
}

/// Renders clause facts as `field: kind payload`, resolving the parent
/// field selection through the node keys.
async fn render_clauses(bowl: &Bowl) -> String {
    let clauses = bowl.scoop::<Query<(Entity, &ClauseFact, &ChildOf)>>().await;
    let fields = bowl.scoop::<Query<(Entity, &FieldSel, &NodeKey)>>().await;
    let field_rows = fields.collect();

    let mut lines: Vec<String> = clauses
        .collect()
        .into_iter()
        .map(|(_, clause, parent)| {
            let field = field_rows
                .iter()
                .find(|(entity, _, _)| *entity == parent.0)
                .map(|(_, field, _)| field.name.as_str())
                .unwrap_or("<no field>");
            let payload = match clause {
                ClauseFact::Where { expr } => format!("where {expr}"),
                ClauseFact::OrderBy { items } => {
                    let items: Vec<String> = items
                        .iter()
                        .map(|item| {
                            let direction = match &item.direction {
                                Some(OrderDirection::Asc) => " asc",
                                Some(OrderDirection::Desc) => " desc",
                                Some(OrderDirection::Variable(_)) => " $var",
                                None => "",
                            };
                            format!("{}{direction}", item.field)
                        })
                        .collect();
                    format!("order by {}", items.join(", "))
                }
                ClauseFact::Limit { expr } => format!("limit {expr}"),
                ClauseFact::Offset { expr } => format!("offset {expr}"),
            };
            format!("{field}: {payload}")
        })
        .collect();
    lines.sort();
    lines.join("\n")
}

#[tokio::test]
async fn clauses_lower_with_expression_trees() {
    let bowl = language_bowl().await;

    insert_source(
        &bowl,
        "valid/imdb-title-basic.dsql",
        &fixture("valid/imdb-title-basic.dsql"),
    )
    .await;
    insert_source(
        &bowl,
        "valid/imdb-scoped-relation-predicate.dsql",
        &fixture("valid/imdb-scoped-relation-predicate.dsql"),
    )
    .await;
    insert_source(
            &bowl,
            "expressions.dsql",
            "query Exprs {\n  users(\n    where .tenant_id == $tenant and (.age >= $$min or .name like \"a%\")\n    order by created_at desc, id\n    limit $$max\n    offset 10\n  ) {\n    id\n  }\n}\n",
        )
        .await;

    insta::assert_snapshot!(render_clauses(&bowl).await);
}

#[tokio::test]
async fn operator_variables_lower_inside_expressions() {
    let bowl = language_bowl().await;

    insert_source(
        &bowl,
        "opvar.dsql",
        "query OpVar {\n  title(where .id $$cmp[==, !=, >] 5) {\n    id\n  }\n}\n",
    )
    .await;

    insta::assert_snapshot!(render_clauses(&bowl).await);
}

/// Pins a language wart: keywords (`limit`, `order`, ...) lex as keyword
/// tokens even after a variable sigil, so `$$limit` parses as an anonymous
/// `$$` followed by a spurious `limit` clause. Candidate fix: contextual
/// keyword handling in the generated lexer.
#[tokio::test]
async fn keyword_named_variables_do_not_parse_as_names() {
    let bowl = language_bowl().await;

    insert_source(
        &bowl,
        "keyword-variable.dsql",
        "query Wart {\n  title(limit $$limit) {\n    id\n  }\n}\n",
    )
    .await;

    insta::assert_snapshot!(render_clauses(&bowl).await);
}

#[tokio::test]
async fn directives_lower_into_facts() {
    let bowl = language_bowl().await;

    insert_source(
            &bowl,
            "directives.dsql",
            "query Annotated @dsql.include_if(condition: $flag) {\n  title @.deprecated(reason: \"old\") {\n    id\n    ...Fields @audit\n  }\n}\nfragment Fields on title {\n  title\n}\n",
        )
        .await;

    let directives = bowl.scoop::<Query<(Entity, &DirectiveFact)>>().await;
    let mut lines: Vec<String> = directives
        .collect()
        .into_iter()
        .map(|(_, directive)| {
            let name = match (&directive.namespace, &directive.member) {
                (Some(namespace), Some(member)) => format!("{namespace}.{member}"),
                (Some(namespace), None) => namespace.clone(),
                (None, Some(member)) => format!(".{member}"),
                (None, None) => "<unnamed>".to_string(),
            };
            let arguments: Vec<String> = directive
                .arguments
                .iter()
                .map(|argument| format!("{}: {}", argument.name, argument.value))
                .collect();
            format!(
                "@{name}({}) shorthand={}",
                arguments.join(", "),
                directive.shorthand
            )
        })
        .collect();
    lines.sort();
    insta::assert_snapshot!(lines.join("\n"));
}

#[tokio::test]
async fn variable_occurrences_become_facts() {
    let bowl = language_bowl().await;

    insert_source(
            &bowl,
            "variables.dsql",
            "query Vars {\n  users(\n    where .tenant_id == $tenant and .age >= $$min\n    order by created_at $$dir\n    limit $$count\n  ) {\n    id\n  }\n}\n",
        )
        .await;

    let variables = bowl.scoop::<Query<(Entity, &VariableUse)>>().await;
    let clauses = bowl.scoop::<Query<(Entity, &ClauseFact, &NodeKey)>>().await;
    let clause_rows = clauses.collect();

    let parents = bowl
        .scoop::<Query<(Entity, &VariableUse, &ChildOf)>>()
        .await;
    let mut lines: Vec<String> = parents
        .collect()
        .into_iter()
        .map(|(_, variable, parent)| {
            let clause = clause_rows
                .iter()
                .find(|(entity, _, _)| *entity == parent.0)
                .map(|(_, clause, _)| match clause {
                    ClauseFact::Where { .. } => "where",
                    ClauseFact::OrderBy { .. } => "order by",
                    ClauseFact::Limit { .. } => "limit",
                    ClauseFact::Offset { .. } => "offset",
                })
                .unwrap_or("<no clause>");
            let name = variable.0.name.as_deref().unwrap_or("<anonymous>");
            format!("{}{name} in {clause}", variable.0.sigil.as_str())
        })
        .collect();
    lines.sort();

    assert_eq!(variables.len(), parents.len(), "every use has a parent");
    insta::assert_snapshot!(lines.join("\n"));
}
