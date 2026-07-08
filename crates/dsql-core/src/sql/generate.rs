//! The SQL generation stage: renders each query plan to PostgreSQL.

use bowl::{Bowl, Commands, Component, DerivedFrom, Entity, Query, Singleton, With};

use super::postgres::{GeneratedSql, PostgresSqlOptions, generate_postgres_sql_with_options};
use crate::catalog::CatalogSnapshot;
use crate::facts::{BelongsToFile, SqlDemand};
use crate::plan::QueryPlanFact;

/// SQL generation options as a fingerprinted singleton fact. A default is
/// installed at registration; projects overwrite it by inserting a new
/// singleton value.
#[derive(Component, Debug, Clone, Copy, Default, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct SqlOptions {
    /// Default `limit` applied to nested collection relations that have no
    /// explicit limit clause, bounding fan-out.
    pub collection_limit: Option<u64>,
}

/// The generated PostgreSQL for one query plan, derived per plan fact by
/// [`generate_sql_facts`], gated on [`SqlDemand`].
#[derive(Component, Debug, Clone, Hash, PartialEq, Eq)]
#[component(hash)]
pub struct GeneratedSqlFact(pub GeneratedSql);

/// Registers the SQL generation stage and installs default [`SqlOptions`].
pub async fn register_sql(bowl: &Bowl) {
    bowl.insert((Singleton::<SqlOptions>::new(), SqlOptions::default()))
        .await;
    bowl.add_system(generate_sql_facts).await;
}

/// Renders each plan against the catalog. Plans that fail to render (a
/// stale plan racing a catalog change; the next settle retires it) emit
/// nothing rather than a broken artifact.
async fn generate_sql_facts(
    _: Query<Entity, With<SqlDemand>>,
    plans: Query<(Entity, &QueryPlanFact, &BelongsToFile, &crate::facts::PlanKey)>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    options: Query<(Entity, &SqlOptions)>,
    mut commands: Commands,
) {
    let (plan_entity, plan, file, plan_key) = plans.item();
    let (catalog_entity, snapshot) = catalog.item();
    let (options_entity, options) = options.item();

    let Ok(sql) = generate_postgres_sql_with_options(
        &plan.0,
        snapshot.catalog(),
        PostgresSqlOptions {
            collection_limit: options.collection_limit,
        },
    ) else {
        return;
    };

    commands.insert((
        DerivedFrom::many([plan_entity, catalog_entity, options_entity]),
        BelongsToFile(file.0),
        *plan_key,
        GeneratedSqlFact(sql),
    ));
}
