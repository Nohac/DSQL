//! The dsql command line, exposed as a library for its integration tests;
//! `main` is pure argument dispatch.

pub mod commands;
#[cfg(debug_assertions)]
pub mod debug;
pub mod render;
