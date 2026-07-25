//! The SQL generation stage: renders each query plan to PostgreSQL.

use crate::schema::dsql_schema;
use bowl::{Commands, Component, DerivedFrom, Entity, Query, Registrar, With};

use super::postgres::{GeneratedSql, PostgresSqlOptions, generate_postgres_sql_with_options};
use crate::catalog::CatalogSnapshot;
use crate::facts::{
    BelongsToFile, DiagnosticCode, DiagnosticFacts, DiagnosticSource, Severity, SqlDemand,
    emit_diagnostic,
};
use crate::plan::{OperationSeed, QueryPlanFact};

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

/// Registers the SQL generation stage; [`crate::language_bowl`] installs
/// the default [`SqlOptions`].
pub fn register_sql(reg: &mut Registrar<'_>) {
    reg.system(generate_sql_facts);
}

/// Renders each plan against the catalog. A plan that fails to render
/// emits an error diagnostic instead of an artifact: a stale plan racing a
/// catalog change retires with its diagnostic at the next settle, while a
/// persistent failure surfaces loudly rather than silently thinning the
/// build tree.
async fn generate_sql_facts(
    _: Query<Entity, With<SqlDemand>>,
    plans: Query<(
        Entity,
        &QueryPlanFact,
        &OperationSeed,
        &BelongsToFile,
        &crate::facts::PlanKey,
    )>,
    catalog: Query<(Entity, &CatalogSnapshot)>,
    options: Query<(Entity, &SqlOptions)>,
    mut commands: Commands<(dsql_schema::GeneratedSql, dsql_schema::Diagnostic)>,
) {
    let (plan_entity, plan, seed, file, plan_key) = plans.item();
    let (catalog_entity, snapshot) = catalog.item();
    let (options_entity, options) = options.item();

    match generate_postgres_sql_with_options(
        &seed.query_name,
        &plan.0,
        snapshot.catalog(),
        PostgresSqlOptions {
            collection_limit: options.collection_limit,
        },
    ) {
        Ok(sql) => {
            commands.insert((
                DerivedFrom::many([plan_entity, catalog_entity, options_entity]),
                BelongsToFile(file.0),
                *plan_key,
                GeneratedSqlFact(sql),
            ));
        }
        Err(error) => emit_diagnostic(
            &mut commands,
            DiagnosticFacts {
                derived_from: DerivedFrom::many([plan_entity, catalog_entity, options_entity]),
                file: file.0,
                span: seed.def_span,
                severity: Severity::Error,
                source: DiagnosticSource::Generate,
                code: DiagnosticCode::SqlGeneration,
                message: format!("query `{}` failed to render SQL: {error}", seed.query_name),
            },
        ),
    }
}
