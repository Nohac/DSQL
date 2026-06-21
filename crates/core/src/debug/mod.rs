#[cfg_attr(debug_assertions, path = "source_allocations_debug.rs")]
pub mod source_allocations;

pub(crate) use source_allocations::{
    record_document_from_rope, record_document_from_string, record_full_text_materialization,
    record_range_text_materialization, record_region_rope_materialization,
};
