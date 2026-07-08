//! Live introspection against a real database, gated on `DATABASE_URL`;
//! without it the test is skipped so the suite stays hermetic.

#[test]
fn introspects_live_database() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set; skipping live introspection test");
        return;
    };

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let metadata = runtime
        .block_on(dsql_introspection::introspect_postgres(&database_url))
        .expect("introspection succeeds");

    assert!(
        !metadata.schemas.is_empty(),
        "a live database exposes at least one schema"
    );
    assert!(
        !metadata.types.is_empty(),
        "type operations come back for built-in types"
    );
}
