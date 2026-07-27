//! Live introspection against a real database, gated on `DATABASE_URL`;
//! without it the test is skipped so the suite stays hermetic.

use dsql_core::catalog::{
    IndexKeyCapability, IndexNullsPosition, IndexOrder, IndexOrderDirection, TypeKey,
    TypeStructureKind,
};
use dsql_core::entities::expression::ComparisonOp;

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

    let catalog = metadata.to_catalog().expect("introspected catalog builds");
    let provider_type = |key: TypeKey| {
        catalog
            .types
            .iter()
            .find(|data_type| data_type.key == key)
            .expect("required PostgreSQL type is introspected")
    };
    let json = provider_type(TypeKey::new("pg_catalog", "json"));
    assert!(
        json.provider.is_some(),
        "fresh introspection marks provider-owned capabilities"
    );
    assert!(!json.capabilities.supports(ComparisonOp::Eq));
    assert!(!json.capabilities.orderable);

    for key in [
        TypeKey::new("pg_catalog", "int4range"),
        TypeKey::new("pg_catalog", "_int4"),
    ] {
        let data_type = provider_type(key);
        assert!(
            data_type.capabilities.supports(ComparisonOp::Eq),
            "{}.{} comparison operators come from its polymorphic family",
            data_type.key.schema,
            data_type.key.name
        );
        assert!(
            data_type.capabilities.orderable,
            "{}.{} ordering comes from its polymorphic btree family",
            data_type.key.schema, data_type.key.name
        );
    }
}

#[test]
fn observatory_comments_and_composite_keys_introspect_end_to_end() {
    let Ok(database_url) = std::env::var("DSQL_OBSERVATORY_DATABASE_URL") else {
        eprintln!("DSQL_OBSERVATORY_DATABASE_URL not set; skipping observatory introspection");
        return;
    };
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let metadata = runtime
        .block_on(dsql_introspection::introspect_postgres(&database_url))
        .expect("observatory introspection succeeds");
    let public = metadata
        .schemas
        .iter()
        .find(|schema| schema.name == "public")
        .expect("public schema");
    let stations = public
        .tables
        .iter()
        .find(|table| table.name == "stations")
        .expect("stations table");
    assert_eq!(
        stations.description.as_deref(),
        Some("Physical stations that collect observations.")
    );
    assert_eq!(
        stations
            .columns
            .iter()
            .find(|column| column.name == "metadata")
            .and_then(|column| column.description.as_deref()),
        Some("Provider-specific station attributes.")
    );
    let foreign_key = stations
        .foreign_keys
        .first()
        .expect("composite network key");
    assert_eq!(foreign_key.columns, ["tenant_id", "network_code"]);
    assert_eq!(foreign_key.references.columns, ["tenant_id", "code"]);

    let active_stations = public
        .tables
        .iter()
        .find(|table| table.name == "active_stations")
        .expect("documented view");
    assert_eq!(
        active_stations.description.as_deref(),
        Some("Stations currently exposed to readers.")
    );
    assert_eq!(
        active_stations
            .columns
            .iter()
            .find(|column| column.name == "code")
            .and_then(|column| column.description.as_deref()),
        Some("Station code within the composite network key.")
    );
    assert_eq!(
        public
            .tables
            .iter()
            .find(|table| table.name == "network_reading_totals")
            .and_then(|table| table.description.as_deref()),
        Some("Precomputed reading counts per network.")
    );

    let readings = public
        .tables
        .iter()
        .find(|table| table.name == "readings")
        .expect("readings table");
    let sensor_time = readings
        .indexes
        .iter()
        .find(|index| index.name.as_deref() == Some("readings_sensor_time_idx"))
        .expect("composite reading index");
    assert_eq!(sensor_time.access_method, "btree");
    assert_eq!(sensor_time.included_columns, ["value"]);
    assert_eq!(
        sensor_time
            .keys
            .iter()
            .map(|key| key.column.as_str())
            .collect::<Vec<_>>(),
        [
            "tenant_id",
            "network_code",
            "station_code",
            "sensor_code",
            "recorded_at",
        ]
    );
    assert_eq!(
        sensor_time.keys[4].order,
        Some(IndexOrder {
            direction: IndexOrderDirection::Desc,
            nulls: IndexNullsPosition::First,
        })
    );

    let sensors = public
        .tables
        .iter()
        .find(|table| table.name == "sensors")
        .expect("sensors table");
    let search = sensors
        .indexes
        .iter()
        .find(|index| index.name.as_deref() == Some("sensors_code_search_idx"))
        .expect("trigram search index");
    assert_eq!(search.access_method, "gin");
    assert!(
        search.keys[0]
            .capabilities
            .contains(&IndexKeyCapability::Like)
    );

    let type_metadata = |key: &TypeKey| {
        metadata
            .types
            .iter()
            .find(|data_type| data_type.key() == *key)
            .expect("structured provider type")
    };
    let station_label = type_metadata(&TypeKey::new("public", "station_label"));
    assert_eq!(station_label.structure.kind, TypeStructureKind::Domain);
    assert_eq!(
        station_label.structure.related_type,
        Some(TypeKey::new("pg_catalog", "text"))
    );
    let text_array = type_metadata(&TypeKey::new("pg_catalog", "_text"));
    assert_eq!(text_array.structure.kind, TypeStructureKind::Array);
    assert_eq!(
        text_array.structure.related_type,
        Some(TypeKey::new("pg_catalog", "text"))
    );
    let bigint_array = type_metadata(&TypeKey::new("pg_catalog", "_int8"));
    assert_eq!(bigint_array.structure.kind, TypeStructureKind::Array);
    assert_eq!(
        bigint_array.structure.related_type,
        Some(TypeKey::new("pg_catalog", "int8"))
    );
    let sensor_row = type_metadata(&TypeKey::new("public", "sensors"));
    assert_eq!(sensor_row.structure.kind, TypeStructureKind::Scalar);
    let sensor_array = type_metadata(&TypeKey::new("public", "_sensors"));
    assert_eq!(sensor_array.structure.kind, TypeStructureKind::Array);
    assert_eq!(
        sensor_array.structure.related_type,
        Some(TypeKey::new("public", "sensors"))
    );
}
