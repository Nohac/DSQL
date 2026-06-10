pub mod catalog;
pub mod definition;
mod diagnostics;
pub mod format;
pub mod lint;
pub mod plan;
pub mod semantic;
pub mod sql;
pub mod syntax;
mod variable_path;

pub use catalog::{
    Catalog, CatalogBuildError, Column, ColumnId, ColumnKey, ColumnMetadata, DataType,
    DatabaseMetadata, FieldCheckResult, ForeignKey, ForeignKeyConstraintMetadata, ForeignKeyId,
    ForeignKeyReferenceMetadata, Index, IndexMetadata, LiteralKind, ObjectType,
    RelationCardinality, RelationField, Schema, SchemaId, SchemaKey, SchemaMetadata, Table,
    TableConstraintKind, TableConstraintMetadata, TableId, TableKey, TableMetadata,
    TableResolution, TypeMetadata, TypeMetadataFile, metadata_from_yaml, metadata_to_yaml,
    table_metadata_from_yaml, table_metadata_to_yaml, type_metadata_file_from_yaml,
    type_metadata_file_to_yaml,
};
pub use definition::{
    DefinitionRecord, DefinitionResolver, ExtractedFile, FragmentKey, FragmentMap, FragmentRecord,
    FragmentSpreadRef, QueryKey, QueryRecord, extract_definitions,
};
pub use diagnostics::{
    CompilerDiagnostic, DsqlDiagnostic, collect_checked_compiler_diagnostics,
    collect_file_compiler_diagnostics, collect_query_compiler_diagnostics,
    extend_compiler_diagnostics, sort_compiler_diagnostics,
};
pub use format::{FormatConfidence, FormatDiagnostic, FormattedText, format_file};
pub use lint::{
    LintDiagnostic, LintDiagnosticKind, LintOptions, LintedDefinition, LintedFile, lint_file,
    lint_file_with_catalog, lint_file_with_options, lint_fragment_definition,
    lint_fragment_definition_with_options, lint_query_definition,
    lint_query_definition_with_options,
};
pub use plan::{
    FilterColumnScope, FilterExpr, FilterLiteral, FragmentPlan, NestedRelation, OrderByPlan,
    PlanDiagnostic, PlanDiagnosticKind, PlannedFile, Projection, QueryPlan, SelectionClauses,
    SelectionPlan, SelectionPlanItem, SortDirectionPlan, SqlParameter, SqlValue, SqlVariantCase,
    plan_file, plan_file_with_catalog, plan_fragment_definition, plan_query_definition,
};
pub use semantic::{
    CheckDiagnostic, CheckDiagnosticKind, CheckError, CheckErrorKind, CheckedDefinition,
    CheckedFile, Interner, LowerDiagnostic, LowerDiagnosticKind, LoweredFile, NameId, NameIndex,
    VariableBinding, VariableBindings, VariableRole, VariableSource, check_file,
    check_file_with_catalog, check_fragment_definition, check_query_definition,
    infer_fragment_variable_bindings, infer_query_variable_bindings, infer_variable_bindings,
    lower_file,
};
pub use sql::{
    GeneratedSql, PostgresSqlOptions, SqlGenerationError, generate_postgres_sql,
    generate_postgres_sql_with_options,
};
pub use syntax::{
    BinaryOp, BinaryOperator, Clause, CstKind, Definition, Diagnostic, DiagnosticCode,
    DiagnosticSource, Expr, Literal, OperatorVariable, ParseResult, PathScope, ScopedPath,
    ScopedPathSegment, Selection, SelectionKind, Severity, SortDirection, SortDirectionExpr,
    SourceFile, SourceSnapshot, SourceText, SyntaxNode, SyntaxToken, SyntaxTree, TextRange, Token,
    ValueVariable, VariableScope, expected_tokens_at, parse_source,
};
pub use variable_path::{InputPathSegment, is_input_path, is_params_path};
