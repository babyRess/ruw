//! ruw is a lightweight Rust web framework facade that builds on Axum.

pub mod api;
pub mod cli;
pub mod db;
pub mod error;

pub use api::{Api, ApiState, ControllerFuture, ReadOnlyResourceController, ResourceController};
pub use axum;
pub use cli::{
    parse_args, run, CliCommand, CliError, GenerateCommand, NewCommand, ResourceCommand,
};
pub use db::connect_sqlite;
pub use error::{ApiError, ApiResult, Problem};
pub use sea_orm;

/// Common imports for building ruw APIs.
pub mod prelude {
    pub use crate::{
        connect_sqlite, Api, ApiError, ApiResult, ApiState, ControllerFuture, Problem,
        ReadOnlyResourceController, ResourceController,
    };

    pub use crate::axum::{extract::State, Json};
}
