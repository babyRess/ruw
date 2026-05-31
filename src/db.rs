//! Database connection helpers for framework-managed persistence.

use sea_orm::DatabaseConnection;

use crate::error::{ApiError, ApiResult};

/// Readable alias for the SeaORM connection handle carried by [`crate::ApiState`].
pub type DbConn = DatabaseConnection;

/// Open a SQLite-backed SeaORM connection for framework state.
///
/// The supplied URL is used only for the connection attempt; public errors are
/// mapped through [`ApiError`] so SQL, paths, and connection strings never reach
/// HTTP problem responses.
pub async fn connect_sqlite(database_url: impl AsRef<str>) -> ApiResult<DatabaseConnection> {
    sea_orm::Database::connect(database_url.as_ref())
        .await
        .map_err(ApiError::from)
}
