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
pub mod infrastructure;
pub mod observability;
pub mod operations;
pub mod web;
