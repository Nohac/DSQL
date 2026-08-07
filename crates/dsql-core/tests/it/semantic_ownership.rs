//! Direct semantic ownership: every nested syntax fact belongs to one
//! dedicated query, fragment, filter, or condition group independently of
//! its nearest structural parent.

use std::collections::{BTreeSet, HashMap};

use bowl::{Bowl, Entity, Eq as BowlEq, Mut, Phase, Query, Related, SystemExt, Where};
use dsql_core::DsqlPlugin;
use dsql_core::catalog::insert_catalog;
use dsql_core::entities::definition::DefDecl;
use dsql_core::entities::document::ParsedFile;
use dsql_core::entities::expansion::SemanticDefinitionKey;
use dsql_core::entities::policy::PolicyDecl;
use dsql_core::entities::variable::VariableUse;
use dsql_core::facts::{
    NodeKey, SemanticMemberOf, SemanticMembers, SemanticRoot, arm_generate_demands,
};
use dsql_core::grammar::parser::{Node, NodeRef};
use dsql_core::plan::QueryPlanFact;
use dsql_core::source::insert_source;
use dsql_core::sql::GeneratedSqlFact;

use crate::{imdb_catalog, render_diagnostic_facts, replace_source_text};

type RelatedVariables<'a> = Related<SemanticMembers, (&'a NodeKey, &'a VariableUse)>;
type SemanticGroupQuery<'a> = Query<(
    Entity,
    &'a SemanticDefinitionKey,
    &'a SemanticRoot,
    RelatedVariables<'a>,
)>;

async fn render_semantic_groups(bowl: &Bowl) -> String {
    let definition_result = bowl.scoop::<Query<(Entity, &DefDecl)>>().await;
    let definitions = definition_result.collect();
    let policy_result = bowl.scoop::<Query<(Entity, &PolicyDecl)>>().await;
    let policies = policy_result.collect();
    let group_result = bowl
        .scoop::<Query<(Entity, &SemanticRoot, Option<&SemanticMembers>)>>()
        .await;
    let groups = group_result.collect();
    let member_result = bowl
        .scoop::<Query<(Entity, &NodeKey, &SemanticMemberOf)>>()
        .await;
    let members = member_result.collect();
    let parsed_result = bowl.scoop::<Query<(Entity, &ParsedFile)>>().await;
    let parsed = parsed_result.collect();

    let mut root_labels = definitions
        .iter()
        .map(|(entity, declaration)| (entity, format!("{} {}", declaration.kind, declaration.name)))
        .chain(policies.iter().map(|(entity, declaration)| {
            (entity, format!("{} {}", declaration.kind, declaration.name))
        }))
        .collect::<HashMap<_, _>>();
    let root_count = root_labels.len();
    let parsed = parsed
        .iter()
        .map(|(entity, parsed)| (entity, parsed))
        .collect::<HashMap<_, _>>();
    let member_keys = members
        .iter()
        .map(|(entity, key, owner)| (entity, (*key, owner.0)))
        .collect::<HashMap<_, _>>();

    let mut lines = Vec::new();
    let mut seen_roots = BTreeSet::new();
    for (group, root, inverse) in &groups {
        let label = root_labels
            .remove(&root.0)
            .unwrap_or_else(|| format!("<missing root {:?}>", root.0));
        seen_roots.insert(root.0);
        let inverse_members = inverse.map_or(&[][..], |members| members.0.as_slice());
        let mut rules = Vec::new();
        for member in inverse_members {
            let (key, owner) = member_keys
                .get(member)
                .copied()
                .expect("every inverse member carries its edge and node key");
            assert_eq!(owner, *group, "member edge and inverse agree");
            let parsed = parsed.get(&key.file).expect("member file is parsed");
            let rule = match parsed.cst.get(NodeRef(key.node)) {
                Node::Rule(rule, _) => format!("{rule:?}@{}", key.node),
                Node::Token(token, _) => format!("token {token:?}@{}", key.node),
            };
            rules.push(rule);
        }
        rules.sort();
        lines.push(format!("{label}: {}", rules.join(", ")));
    }
    lines.sort();

    assert_eq!(
        seen_roots.len(),
        root_count,
        "every semantic root has one group"
    );
    assert!(
        root_labels.is_empty(),
        "every root was represented exactly once"
    );
    assert!(
        definitions
            .iter()
            .all(|(entity, _)| !member_keys.contains_key(entity))
            && policies
                .iter()
                .all(|(entity, _)| !member_keys.contains_key(entity)),
        "semantic roots never become members, even if roots are nested later"
    );
    lines.join("\n")
}

async fn semantic_group_observer(
    groups: SemanticGroupQuery<'_>,
    _roots: Query<(Entity, &DefDecl), Where<BowlEq<SemanticDefinitionKey>>>,
) {
    let _ = groups.item();
}

async fn observed_bowl() -> Bowl {
    let bowl = Bowl::builder()
        .plugin(DsqlPlugin)
        .system(semantic_group_observer.run_during(Phase::Complete))
        .build();
    dsql_core::install_default_singletons(&bowl).await;
    bowl
}

async fn observer_runs(bowl: &Bowl) -> u64 {
    let _settled = bowl.scoop::<Query<(Entity, &SemanticRoot)>>().await;
    bowl.profile_all()
        .await
        .into_iter()
        .find(|entry| entry.name.ends_with("semantic_group_observer"))
        .map_or(0, |entry| entry.runs)
}

async fn terminal_state(bowl: &Bowl) -> (String, Vec<(String, String)>, String) {
    let graph = render_semantic_groups(bowl).await;
    let sql_result = bowl.scoop::<Query<(Entity, &GeneratedSqlFact)>>().await;
    let mut sql = sql_result
        .collect()
        .iter()
        .map(|(_, generated)| {
            (
                generated.0.operation_name.clone(),
                generated.0.compact_sql.clone(),
            )
        })
        .collect::<Vec<_>>();
    sql.sort();
    let diagnostics = render_diagnostic_facts(bowl).await;
    (graph, sql, diagnostics)
}

#[tokio::test]
async fn lowering_builds_direct_semantic_groups() {
    let bowl = dsql_core::language_bowl().await;
    insert_source(
        &bowl,
        "queries.dsql",
        indoc::indoc! {r#"
            fragment Bits on title {
              id
            }
            query First @dsql.deprecated(reason: "old") {
              title(filter Titles where .id == $id limit %limit) {
                ...Bits @dsql.deprecated
                movie_info_idx | aggregate { count }
              }
            }
        "#},
    )
    .await;
    insert_source(
        &bowl,
        "policies.dsql",
        indoc::indoc! {r#"
            condition Enabled { where true }
            filter Titles on public::title {
              apply where Enabled
              where .id > $:minimum_id
              field production_year where true
            }
        "#},
    )
    .await;

    insta::assert_snapshot!(render_semantic_groups(&bowl).await);
}

#[tokio::test]
async fn semantic_group_edits_are_owner_granular_and_cold_equivalent() {
    const INITIAL_FIRST: &str = "query First {\n  title(limit %first_limit) {\n    id\n  }\n}\n";
    const FINAL_FIRST: &str =
        "query First {\n  title(limit %first_limit) {\n    id\n    title\n  }\n}\n";
    const SECOND: &str = "query Second {\n  title(limit %second_limit) {\n    id\n  }\n}\n";

    let incremental = observed_bowl().await;
    insert_catalog(&incremental, imdb_catalog()).await;
    arm_generate_demands(&incremental).await;
    let first = insert_source(&incremental, "first.dsql", INITIAL_FIRST).await;
    insert_source(&incremental, "second.dsql", SECOND).await;
    let initial_graph = render_semantic_groups(&incremental).await;
    let sibling_before = initial_graph
        .lines()
        .find(|line| line.starts_with("query Second:"))
        .expect("sibling group exists")
        .to_string();
    let baseline_runs = observer_runs(&incremental).await;
    assert_eq!(
        incremental
            .scoop::<Query<(Entity, &QueryPlanFact)>>()
            .await
            .len(),
        2,
        "root-anchored plans exist before the membership edit"
    );

    let variable_result = incremental.scoop::<Query<(Entity, &VariableUse)>>().await;
    let first_variable = variable_result
        .collect()
        .iter()
        .find_map(|(entity, variable)| {
            (variable.0.name.as_deref() == Some("first_limit")).then_some(*entity)
        })
        .expect("the first query has a named variable");
    drop(variable_result);
    let mutable_variable_result = incremental
        .scoop::<Query<(Entity, Mut<VariableUse>)>>()
        .await;
    for (entity, variable) in mutable_variable_result.collect() {
        if entity == first_variable {
            variable
                .with_latest(|variable| {
                    variable.0.name = Some("first_limit_probe".to_string());
                })
                .await;
        }
    }
    drop(mutable_variable_result);
    assert_eq!(
        observer_runs(&incremental).await - baseline_runs,
        1,
        "a projected member change reaches exactly its semantic group"
    );
    let before_membership_runs = observer_runs(&incremental).await;

    replace_source_text(&incremental, first, "    id\n", "    id\n    title\n").await;
    let incremental_state = terminal_state(&incremental).await;
    let sibling_after = incremental_state
        .0
        .lines()
        .find(|line| line.starts_with("query Second:"))
        .expect("sibling group remains")
        .to_string();
    assert_eq!(sibling_after, sibling_before);
    assert_eq!(
        observer_runs(&incremental).await - before_membership_runs,
        1,
        "a membership change reaches only the edited root's group observer"
    );
    assert_eq!(
        incremental
            .scoop::<Query<(Entity, &QueryPlanFact)>>()
            .await
            .len(),
        2,
        "root-anchored plans remain current after membership changes"
    );

    let cold = observed_bowl().await;
    insert_catalog(&cold, imdb_catalog()).await;
    arm_generate_demands(&cold).await;
    insert_source(&cold, "first.dsql", FINAL_FIRST).await;
    insert_source(&cold, "second.dsql", SECOND).await;
    assert_eq!(
        terminal_state(&cold).await,
        incremental_state,
        "cold and incrementally reached terminal fact graphs agree"
    );

    replace_source_text(&incremental, first, FINAL_FIRST, "").await;
    let after_removal = render_semantic_groups(&incremental).await;
    assert!(!after_removal.contains("query First:"));
    assert!(after_removal.contains(&sibling_before));
}
