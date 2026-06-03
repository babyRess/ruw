//! Thin Axum facade and framework state for ruw applications.

use std::{fmt::Display, future::Future, pin::Pin, sync::Arc};

use axum::{
    extract::{
        rejection::{JsonRejection, PathRejection},
        FromRequest, FromRequestParts, Path, Request, State,
    },
    http::{header::LOCATION, request::Parts, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use sea_orm::DatabaseConnection;
use serde::{de::DeserializeOwned, Serialize};

use crate::{
    db,
    error::{ApiError, ApiResult},
};

/// Framework-owned state carried by the default [`Api`] builder.
///
/// The private inner allocation keeps the public state type stable while later
/// slices add framework-managed resources such as database handles.
#[derive(Clone, Debug, Default)]
pub struct ApiState {
    inner: Arc<ApiStateInner>,
}

#[derive(Debug, Default)]
struct ApiStateInner {
    database: Option<DatabaseConnection>,
}

impl ApiState {
    /// Build framework state from an existing SeaORM database connection.
    pub fn with_database(database: DatabaseConnection) -> Self {
        Self {
            inner: Arc::new(ApiStateInner {
                database: Some(database),
            }),
        }
    }

    /// Open a SQLite database connection and store it in framework state.
    pub async fn connect_sqlite(database_url: impl AsRef<str>) -> ApiResult<Self> {
        db::connect_sqlite(database_url)
            .await
            .map(Self::with_database)
    }

    /// Open SQLite state only when a database URL is configured.
    ///
    /// `None` or a blank URL returns default framework state. This keeps apps
    /// that support optional persistence from having to branch around
    /// [`ApiState`] setup themselves.
    pub async fn connect_optional_sqlite(database_url: Option<impl AsRef<str>>) -> ApiResult<Self> {
        let Some(database_url) = database_url else {
            return Ok(Self::default());
        };

        if database_url.as_ref().trim().is_empty() {
            Ok(Self::default())
        } else {
            Self::connect_sqlite(database_url).await
        }
    }

    /// Access the configured framework-managed database connection.
    ///
    /// A missing database is treated as a framework state error rather than a
    /// panic so handlers can continue using the normal [`ApiError`] response
    /// seam.
    pub fn database(&self) -> ApiResult<&DatabaseConnection> {
        self.inner
            .database
            .as_ref()
            .ok_or_else(|| ApiError::missing_framework_state("database"))
    }
}

/// Boxed `Send` future returned by [`ResourceController`] methods.
pub type ControllerFuture<'a, T> = Pin<Box<dyn Future<Output = ApiResult<T>> + Send + 'a>>;

/// Ordinary-Rust CRUD controller contract adapted by [`Api::resource`].
///
/// Controllers stay macro-light: implement this trait, return boxed futures,
/// and let the facade handle Axum path/JSON extraction plus HTTP response
/// conversion.
pub trait ResourceController<S = ApiState>: Clone + Send + Sync + 'static
where
    S: Clone + Send + Sync + 'static,
{
    /// Path identifier parsed by Axum's typed [`Path`] extractor.
    type Id: DeserializeOwned + Display + Send + Sync + 'static;
    /// Public JSON resource returned to callers.
    type Resource: Serialize + Send + Sync + 'static;
    /// JSON body used to create a resource.
    type Create: DeserializeOwned + Send + 'static;
    /// JSON body used to update a resource.
    type Update: DeserializeOwned + Send + 'static;

    /// List resources for the collection route.
    fn index(&self, state: S) -> ControllerFuture<'_, Vec<Self::Resource>>;

    /// Fetch one resource by id. `None` is mapped through the not-found seam.
    fn show(&self, state: S, id: Self::Id) -> ControllerFuture<'_, Option<Self::Resource>>;

    /// Create a resource from the typed JSON body.
    fn create(&self, state: S, input: Self::Create) -> ControllerFuture<'_, Self::Resource>;

    /// Validate a typed create DTO after JSON extraction and before mutation.
    fn validate_create(&self, _input: &Self::Create) -> ApiResult<()> {
        Ok(())
    }

    /// Update one resource by id. `None` is mapped through the not-found seam.
    fn update(
        &self,
        state: S,
        id: Self::Id,
        input: Self::Update,
    ) -> ControllerFuture<'_, Option<Self::Resource>>;

    /// Validate a typed update DTO after JSON extraction and before mutation.
    fn validate_update(&self, _input: &Self::Update) -> ApiResult<()> {
        Ok(())
    }

    /// Delete one resource by id. `false` is mapped through the not-found seam.
    fn delete(&self, state: S, id: Self::Id) -> ControllerFuture<'_, bool>;

    /// Return the id used to build a successful create `Location` header.
    fn resource_id(&self, resource: &Self::Resource) -> Self::Id;
}

/// Ordinary-Rust read-only controller contract adapted by [`Api::read_only_resource`].
///
/// Use this when a resource exposes collection/member reads but should not
/// commit to create, update, or delete behavior.
pub trait ReadOnlyResourceController<S = ApiState>: Clone + Send + Sync + 'static
where
    S: Clone + Send + Sync + 'static,
{
    /// Path identifier parsed by Axum's typed [`Path`] extractor.
    type Id: DeserializeOwned + Display + Send + Sync + 'static;
    /// Public JSON resource returned to callers.
    type Resource: Serialize + Send + Sync + 'static;

    /// List resources for the collection route.
    fn index(&self, state: S) -> ControllerFuture<'_, Vec<Self::Resource>>;

    /// Fetch one resource by id. `None` is mapped through the not-found seam.
    fn show(&self, state: S, id: Self::Id) -> ControllerFuture<'_, Option<Self::Resource>>;
}

struct ResourcePath<T>(T);

impl<T, S> FromRequestParts<S> for ResourcePath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(value) = Path::<T>::from_request_parts(parts, state)
            .await
            .map_err(map_path_rejection)?;

        Ok(Self(value))
    }
}

struct ResourceJson<T>(T);

impl<T, S> FromRequest<S> for ResourceJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(req, state)
            .await
            .map_err(map_json_rejection)?;

        Ok(Self(value))
    }
}

fn map_path_rejection(rejection: PathRejection) -> ApiError {
    ApiError::bad_request("path", path_rejection_category(&rejection))
}

fn path_rejection_category(rejection: &PathRejection) -> &'static str {
    match rejection {
        PathRejection::FailedToDeserializePathParams(_) => "failed_to_deserialize_path_params",
        PathRejection::MissingPathParams(_) => "missing_path_params",
        _ => "unknown_path_rejection",
    }
}

fn map_json_rejection(rejection: JsonRejection) -> ApiError {
    ApiError::bad_request("json", json_rejection_category(&rejection))
}

fn json_rejection_category(rejection: &JsonRejection) -> &'static str {
    match rejection {
        JsonRejection::JsonDataError(_) => "json_data_error",
        JsonRejection::JsonSyntaxError(_) => "json_syntax_error",
        JsonRejection::MissingJsonContentType(_) => "missing_json_content_type",
        JsonRejection::BytesRejection(_) => "bytes_rejection",
        _ => "unknown_json_rejection",
    }
}

/// A small builder facade over [`axum::Router`].
///
/// `Api` keeps Axum's typed state during route assembly and only applies the
/// concrete state when [`Api::into_router`] is called.
pub struct Api<S = ApiState> {
    router: axum::Router<S>,
    state: S,
}

impl<S> Api<S>
where
    S: Clone + Send + Sync + 'static,
{
    /// Create an API builder with framework/application state.
    pub fn new(state: S) -> Self {
        Self {
            router: axum::Router::new(),
            state,
        }
    }

    /// Register a typed Axum method router at `path`.
    pub fn route(mut self, path: &str, routes: axum::routing::MethodRouter<S>) -> Self {
        self.router = self.router.route(path, routes);
        self
    }

    /// Merge a raw Axum router into this API builder.
    pub fn merge(mut self, router: axum::Router<S>) -> Self {
        self.router = self.router.merge(router);
        self
    }

    /// Register conventional CRUD routes for a [`ResourceController`].
    ///
    /// The collection path receives `GET` and `POST`; the member path appends
    /// Axum 0.8 `{id}` syntax and receives `GET`, `PATCH`, and `DELETE`.
    pub fn resource<C>(mut self, path: &str, controller: C) -> Self
    where
        C: ResourceController<S>,
    {
        let collection_path = normalize_resource_path(path);
        let member_path = member_resource_path(&collection_path);

        let index_controller = controller.clone();
        let create_controller = controller.clone();
        let show_controller = controller.clone();
        let update_controller = controller.clone();
        let delete_controller = controller;
        let create_base_path = collection_path.clone();

        let collection_routes = axum::routing::get(move |State(state): State<S>| {
            let controller = index_controller.clone();
            async move { resource_index(controller, state).await }
        })
        .post(
            move |State(state): State<S>, ResourceJson(input): ResourceJson<C::Create>| {
                let controller = create_controller.clone();
                let base_path = create_base_path.clone();
                async move { resource_create(controller, base_path, state, input).await }
            },
        );

        let member_routes = axum::routing::get(
            move |ResourcePath(id): ResourcePath<C::Id>, State(state): State<S>| {
                let controller = show_controller.clone();
                async move { resource_show(controller, state, id).await }
            },
        )
        .patch(
            move |ResourcePath(id): ResourcePath<C::Id>,
                  State(state): State<S>,
                  ResourceJson(input): ResourceJson<C::Update>| {
                let controller = update_controller.clone();
                async move { resource_update(controller, state, id, input).await }
            },
        )
        .delete(
            move |ResourcePath(id): ResourcePath<C::Id>, State(state): State<S>| {
                let controller = delete_controller.clone();
                async move { resource_delete(controller, state, id).await }
            },
        );

        self.router = self
            .router
            .route(&collection_path, collection_routes)
            .route(&member_path, member_routes);
        self
    }

    /// Register read-only collection and member routes for a controller.
    ///
    /// The collection path receives `GET`; the member path appends Axum 0.8
    /// `{id}` syntax and receives `GET`.
    pub fn read_only_resource<C>(mut self, path: &str, controller: C) -> Self
    where
        C: ReadOnlyResourceController<S>,
    {
        let collection_path = normalize_resource_path(path);
        let member_path = member_resource_path(&collection_path);

        let index_controller = controller.clone();
        let show_controller = controller;

        let collection_routes = axum::routing::get(move |State(state): State<S>| {
            let controller = index_controller.clone();
            async move { read_only_resource_index(controller, state).await }
        });

        let member_routes = axum::routing::get(
            move |ResourcePath(id): ResourcePath<C::Id>, State(state): State<S>| {
                let controller = show_controller.clone();
                async move { read_only_resource_show(controller, state, id).await }
            },
        );

        self.router = self
            .router
            .route(&collection_path, collection_routes)
            .route(&member_path, member_routes);
        self
    }

    /// Finalize this API into a normal Axum router by applying the owned state.
    pub fn into_router(self) -> axum::Router {
        self.router.with_state(self.state)
    }

    /// Serve this API on an existing Tokio TCP listener.
    pub async fn serve(self, listener: tokio::net::TcpListener) -> std::io::Result<()> {
        axum::serve(listener, self.into_router()).await
    }
}

fn normalize_resource_path(path: &str) -> String {
    let path = path.trim().trim_end_matches('/').trim_start_matches('/');

    if path.is_empty() {
        "/".to_owned()
    } else {
        format!("/{path}")
    }
}

fn member_resource_path(collection_path: &str) -> String {
    if collection_path == "/" {
        "/{id}".to_owned()
    } else {
        format!("{collection_path}/{{id}}")
    }
}

async fn resource_index<C, S>(controller: C, state: S) -> ApiResult<Json<Vec<C::Resource>>>
where
    C: ResourceController<S>,
    S: Clone + Send + Sync + 'static,
{
    controller.index(state).await.map(Json)
}

async fn resource_show<C, S>(controller: C, state: S, id: C::Id) -> ApiResult<Json<C::Resource>>
where
    C: ResourceController<S>,
    S: Clone + Send + Sync + 'static,
{
    match controller.show(state, id).await? {
        Some(resource) => Ok(Json(resource)),
        None => Err(resource_not_found("show")),
    }
}

async fn read_only_resource_index<C, S>(
    controller: C,
    state: S,
) -> ApiResult<Json<Vec<C::Resource>>>
where
    C: ReadOnlyResourceController<S>,
    S: Clone + Send + Sync + 'static,
{
    controller.index(state).await.map(Json)
}

async fn read_only_resource_show<C, S>(
    controller: C,
    state: S,
    id: C::Id,
) -> ApiResult<Json<C::Resource>>
where
    C: ReadOnlyResourceController<S>,
    S: Clone + Send + Sync + 'static,
{
    match controller.show(state, id).await? {
        Some(resource) => Ok(Json(resource)),
        None => Err(resource_not_found("show")),
    }
}

async fn resource_create<C, S>(
    controller: C,
    base_path: String,
    state: S,
    input: C::Create,
) -> ApiResult<Response>
where
    C: ResourceController<S>,
    S: Clone + Send + Sync + 'static,
{
    controller.validate_create(&input)?;

    let resource = controller.create(state, input).await?;
    let location = resource_location(&base_path, controller.resource_id(&resource))?;
    let mut response = (StatusCode::CREATED, Json(resource)).into_response();
    response.headers_mut().insert(LOCATION, location);

    Ok(response)
}

async fn resource_update<C, S>(
    controller: C,
    state: S,
    id: C::Id,
    input: C::Update,
) -> ApiResult<Json<C::Resource>>
where
    C: ResourceController<S>,
    S: Clone + Send + Sync + 'static,
{
    controller.validate_update(&input)?;

    match controller.update(state, id, input).await? {
        Some(resource) => Ok(Json(resource)),
        None => Err(resource_not_found("update")),
    }
}

async fn resource_delete<C, S>(controller: C, state: S, id: C::Id) -> ApiResult<StatusCode>
where
    C: ResourceController<S>,
    S: Clone + Send + Sync + 'static,
{
    if controller.delete(state, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(resource_not_found("delete"))
    }
}

fn resource_location<I>(base_path: &str, id: I) -> ApiResult<HeaderValue>
where
    I: Display,
{
    let location = if base_path == "/" {
        format!("/{id}")
    } else {
        format!("{}/{id}", base_path.trim_end_matches('/'))
    };

    HeaderValue::from_str(&location).map_err(|error| {
        tracing::error!(
            error = %error,
            "controller resource id could not be encoded as a Location header"
        );
        ApiError::Internal
    })
}

fn resource_not_found(operation: &'static str) -> ApiError {
    ApiError::resource_not_found(operation)
}

#[cfg(test)]
mod tests {
    use super::{Api, ApiState};
    use crate::ApiError;
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

    #[test]
    fn api_state_is_defaultable_and_cloneable() {
        let state = ApiState::default();
        let _cloned = state.clone();
    }

    #[test]
    fn api_state_database_accessor_reports_missing_database() {
        let error = ApiState::default()
            .database()
            .expect_err("default ApiState should not have a database");

        assert!(matches!(error, ApiError::Internal));
    }

    #[tokio::test]
    async fn api_state_with_database_exposes_configured_database_to_clones() {
        let database = crate::db::connect_sqlite("sqlite::memory:")
            .await
            .expect("in-memory SQLite should connect");
        let state = ApiState::with_database(database);

        assert!(state.database().is_ok());
        assert!(state.clone().database().is_ok());
    }

    #[tokio::test]
    async fn api_state_connect_sqlite_opens_usable_database() {
        let state = ApiState::connect_sqlite("sqlite::memory:")
            .await
            .expect("in-memory SQLite should connect");

        state
            .database()
            .expect("database should be configured")
            .execute(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT 1".to_owned(),
            ))
            .await
            .expect("database should execute a simple query");
    }

    #[tokio::test]
    async fn api_state_connect_optional_sqlite_defaults_without_url() {
        let state = ApiState::connect_optional_sqlite(None::<&str>)
            .await
            .expect("missing database URL should produce default state");

        assert!(matches!(state.database(), Err(ApiError::Internal)));
    }

    #[tokio::test]
    async fn api_state_connect_optional_sqlite_defaults_for_blank_url() {
        let state = ApiState::connect_optional_sqlite(Some("   "))
            .await
            .expect("blank database URL should produce default state");

        assert!(matches!(state.database(), Err(ApiError::Internal)));
    }

    #[tokio::test]
    async fn api_state_connect_optional_sqlite_opens_configured_database() {
        let state = ApiState::connect_optional_sqlite(Some("sqlite::memory:"))
            .await
            .expect("configured database URL should connect");

        state
            .database()
            .expect("database should be configured")
            .execute(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT 1".to_owned(),
            ))
            .await
            .expect("database should execute a simple query");
    }

    #[test]
    fn api_finalizes_into_axum_router() {
        let _router: axum::Router = Api::new(ApiState::default()).into_router();
    }
}
