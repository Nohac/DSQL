mod check;
mod error;
mod lower;

pub use check::{check_file, check_file_with_catalog};
pub use error::{CheckError, CheckErrorKind, CheckedFile};
pub use lower::{Interner, LoweredFile, NameId, NameIndex, lower_file};
