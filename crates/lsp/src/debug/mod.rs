#[cfg_attr(debug_assertions, path = "source_allocation_stats_debug.rs")]
mod source_allocation_stats;

pub(crate) use source_allocation_stats::{code_action, configure_capabilities, execute_command};
