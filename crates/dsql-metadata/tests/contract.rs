#![cfg(feature = "render")]

use dsql_metadata::{DefinitionKind, ResultFieldKind, ResultValueShape, SqlDialect, WireEncoding};

macro_rules! assert_enum_contract {
    ($type:ty, $($variant:path => $spelling:literal),+ $(,)?) => {{
        $(
            assert_eq!($variant.as_ref(), $spelling);

            let serialized = facet_json::to_string(&$variant).expect("enum serializes");
            assert_eq!(serialized, concat!("\"", $spelling, "\""));

            let parsed: $type =
                facet_json::from_str(concat!("\"", $spelling, "\"")).expect("enum parses");
            assert_eq!(parsed, $variant);
        )+

        let unknown = facet_json::from_str::<$type>("\"legacy\"");
        assert!(unknown.is_err(), "unknown enum variant must be rejected");
    }};
}

#[test]
fn metadata_enums_have_strict_wire_spellings() {
    assert_enum_contract!(
        DefinitionKind,
        DefinitionKind::Query => "query",
        DefinitionKind::Fragment => "fragment",
    );
    assert_enum_contract!(SqlDialect, SqlDialect::Postgres => "postgres");
    assert_enum_contract!(
        ResultFieldKind,
        ResultFieldKind::Scalar => "scalar",
        ResultFieldKind::Object => "object",
        ResultFieldKind::Array => "array",
    );
    assert_enum_contract!(
        ResultValueShape,
        ResultValueShape::Scalar => "scalar",
        ResultValueShape::DatabaseArray => "database_array",
        ResultValueShape::Object => "object",
    );
    assert_enum_contract!(
        WireEncoding,
        WireEncoding::Uuid => "uuid",
        WireEncoding::Text => "text",
        WireEncoding::Timestamptz => "timestamptz",
        WireEncoding::Integer => "integer",
        WireEncoding::BigInteger => "big_integer",
        WireEncoding::Numeric => "numeric",
        WireEncoding::Float => "float",
        WireEncoding::Boolean => "boolean",
        WireEncoding::Json => "json",
        WireEncoding::TextCast => "text_cast",
        WireEncoding::Unsupported => "unsupported",
    );
    assert_eq!(WireEncoding::TextCast.as_str(), "text_cast");
}

#[test]
fn generated_typescript_emits_shared_contracts_once() {
    let typescript = dsql_metadata::build_manifest_typescript();
    for declaration in [
        "export type DefinitionKind ",
        "export type ResultFieldKind ",
        "export type ResultValueShape ",
        "export type SqlDialect ",
        "export type WireEncoding ",
        "export interface InputField ",
        "export interface ResultField ",
        "export interface WireMetadata ",
    ] {
        assert_eq!(
            typescript.matches(declaration).count(),
            1,
            "`{declaration}` should be emitted once"
        );
    }
}
