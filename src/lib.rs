//! ruw is a lightweight Rust web framework facade that builds on Axum.

pub mod api;
pub mod db;
pub mod error;

pub use api::{Api, ApiState, ControllerFuture, ResourceController};
pub use axum;
pub use db::connect_sqlite;
pub use error::{ApiError, ApiResult, Problem};
pub use sea_orm;

/// Common imports for building ruw APIs.
pub mod prelude {
    pub use crate::{
        connect_sqlite, Api, ApiError, ApiResult, ApiState, ControllerFuture, Problem,
        ResourceController,
    };

    pub use crate::axum::{extract::State, Json};
}
