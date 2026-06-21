use facet::Facet;
use std::sync::atomic::{AtomicU64, Ordering};

static DOCUMENTS_FROM_STRING: AtomicU64 = AtomicU64::new(0);
static DOCUMENTS_FROM_STRING_BYTES: AtomicU64 = AtomicU64::new(0);
static DOCUMENTS_FROM_ROPE: AtomicU64 = AtomicU64::new(0);
static DOCUMENTS_FROM_ROPE_BYTES: AtomicU64 = AtomicU64::new(0);
static FULL_TEXT_MATERIALIZATIONS: AtomicU64 = AtomicU64::new(0);
static FULL_TEXT_MATERIALIZATION_BYTES: AtomicU64 = AtomicU64::new(0);
static RANGE_TEXT_MATERIALIZATIONS: AtomicU64 = AtomicU64::new(0);
static RANGE_TEXT_MATERIALIZATION_BYTES: AtomicU64 = AtomicU64::new(0);
static REGION_ROPE_MATERIALIZATIONS: AtomicU64 = AtomicU64::new(0);
static REGION_ROPE_MATERIALIZATION_BYTES: AtomicU64 = AtomicU64::new(0);

/// Counters for source-level materialization decisions.
///
/// These counters track explicit DSQL source representation work, not every
/// heap allocation performed by dependencies or the global allocator.
#[derive(Clone, Copy, Debug, Default, Facet, PartialEq, Eq)]
pub struct SourceAllocationStats {
    pub documents_from_string: u64,
    pub documents_from_string_bytes: u64,
    pub documents_from_rope: u64,
    pub documents_from_rope_bytes: u64,
    pub full_text_materializations: u64,
    pub full_text_materialization_bytes: u64,
    pub range_text_materializations: u64,
    pub range_text_materialization_bytes: u64,
    pub region_rope_materializations: u64,
    pub region_rope_materialization_bytes: u64,
}

/// Returns the current source materialization counters.
pub fn source_allocation_stats() -> SourceAllocationStats {
    SourceAllocationStats {
        documents_from_string: DOCUMENTS_FROM_STRING.load(Ordering::Relaxed),
        documents_from_string_bytes: DOCUMENTS_FROM_STRING_BYTES.load(Ordering::Relaxed),
        documents_from_rope: DOCUMENTS_FROM_ROPE.load(Ordering::Relaxed),
        documents_from_rope_bytes: DOCUMENTS_FROM_ROPE_BYTES.load(Ordering::Relaxed),
        full_text_materializations: FULL_TEXT_MATERIALIZATIONS.load(Ordering::Relaxed),
        full_text_materialization_bytes: FULL_TEXT_MATERIALIZATION_BYTES.load(Ordering::Relaxed),
        range_text_materializations: RANGE_TEXT_MATERIALIZATIONS.load(Ordering::Relaxed),
        range_text_materialization_bytes: RANGE_TEXT_MATERIALIZATION_BYTES.load(Ordering::Relaxed),
        region_rope_materializations: REGION_ROPE_MATERIALIZATIONS.load(Ordering::Relaxed),
        region_rope_materialization_bytes: REGION_ROPE_MATERIALIZATION_BYTES
            .load(Ordering::Relaxed),
    }
}

/// Resets the source materialization counters.
pub fn reset_source_allocation_stats() {
    DOCUMENTS_FROM_STRING.store(0, Ordering::Relaxed);
    DOCUMENTS_FROM_STRING_BYTES.store(0, Ordering::Relaxed);
    DOCUMENTS_FROM_ROPE.store(0, Ordering::Relaxed);
    DOCUMENTS_FROM_ROPE_BYTES.store(0, Ordering::Relaxed);
    FULL_TEXT_MATERIALIZATIONS.store(0, Ordering::Relaxed);
    FULL_TEXT_MATERIALIZATION_BYTES.store(0, Ordering::Relaxed);
    RANGE_TEXT_MATERIALIZATIONS.store(0, Ordering::Relaxed);
    RANGE_TEXT_MATERIALIZATION_BYTES.store(0, Ordering::Relaxed);
    REGION_ROPE_MATERIALIZATIONS.store(0, Ordering::Relaxed);
    REGION_ROPE_MATERIALIZATION_BYTES.store(0, Ordering::Relaxed);
}

#[inline]
fn record_source_allocation(count: &AtomicU64, bytes_total: &AtomicU64, bytes: usize) {
    count.fetch_add(1, Ordering::Relaxed);
    bytes_total.fetch_add(bytes as u64, Ordering::Relaxed);
}

#[inline]
pub(crate) fn record_document_from_string(bytes: usize) {
    record_source_allocation(&DOCUMENTS_FROM_STRING, &DOCUMENTS_FROM_STRING_BYTES, bytes);
}

#[inline]
pub(crate) fn record_document_from_rope(bytes: usize) {
    record_source_allocation(&DOCUMENTS_FROM_ROPE, &DOCUMENTS_FROM_ROPE_BYTES, bytes);
}

#[inline]
pub(crate) fn record_full_text_materialization(bytes: usize) {
    record_source_allocation(
        &FULL_TEXT_MATERIALIZATIONS,
        &FULL_TEXT_MATERIALIZATION_BYTES,
        bytes,
    );
}

#[inline]
pub(crate) fn record_range_text_materialization(bytes: usize) {
    record_source_allocation(
        &RANGE_TEXT_MATERIALIZATIONS,
        &RANGE_TEXT_MATERIALIZATION_BYTES,
        bytes,
    );
}

#[inline]
pub(crate) fn record_region_rope_materialization(bytes: usize) {
    record_source_allocation(
        &REGION_ROPE_MATERIALIZATIONS,
        &REGION_ROPE_MATERIALIZATION_BYTES,
        bytes,
    );
}
