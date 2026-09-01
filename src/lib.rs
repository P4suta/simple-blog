//! Application library for simple-blog.
//!
//! Dependencies point inward: `web` and `infrastructure` adapt external systems,
//! `application` coordinates use-cases, and `domain` owns business rules.

#[cfg(panic = "abort")]
compile_error!("simple-blog requires panic=unwind so one failed request cannot terminate the CMS");

pub mod application;
pub mod cli;
pub mod config;
pub mod domain;
mod durable_fs;
pub mod i18n;
pub mod infrastructure;
pub mod materialize;
pub mod observability;
pub mod operations;
pub mod portable;
pub mod release;
pub mod web;
