mod check;
mod error;
mod lower;
mod variables;

pub use check::{
    check_file, check_file_with_catalog, check_fragment_definition, check_query_definition,
    duplicate_fragment_errors,
};
pub use error::{
    CheckDiagnostic, CheckDiagnosticKind, CheckError, CheckErrorKind, CheckedDefinition,
    CheckedFile,
};
pub use lower::{
    Interner, LowerDiagnostic, LowerDiagnosticKind, LoweredFile, NameId, NameIndex, lower_file,
};
pub use variables::{
    VariableBinding, VariableBindings, VariableRole, VariableSource,
    infer_fragment_variable_bindings, infer_query_variable_bindings, infer_variable_bindings,
};
