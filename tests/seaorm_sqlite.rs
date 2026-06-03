#[path = "support/todos_entity.rs"]
mod todos_entity;

use ruw::{
    axum::{
        body::{to_bytes, Body},
        http::{
            header::{CONTENT_TYPE, LOCATION},
            Method, Request, StatusCode,
        },
        response::Response,
        routing::get,
        Router,
    },
    prelude::*,
    sea_orm::{
        ActiveModelTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait,
        QueryOrder, Set, Statement,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn sqlite_state_with_todos_schema() -> ApiState {
    let state = ApiState::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite database should connect through framework state");

    state
        .database()
        .expect("framework state should expose the configured database")
        .execute(Statement::from_string(
            DatabaseBackend::Sqlite,
            r#"
            CREATE TABLE todos (
                id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
                title TEXT NOT NULL,
                completed BOOLEAN NOT NULL
            )
            "#
            .to_owned(),
        ))
        .await
        .expect("todos schema should be created in the isolated SQLite database");

    state
}

async fn sqlite_database_with_todos_schema() -> DatabaseConnection {
    sqlite_state_with_todos_schema()
        .await
        .database()
        .expect("framework state should expose the configured database")
        .clone()
}

#[derive(Clone, Debug, Deserialize)]
struct CreateTodo {
    title: String,
}

#[derive(Clone, Debug, Deserialize)]
struct UpdateTodo {
    title: Option<String>,
    completed: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
struct TodoResponse {
    id: i32,
    title: String,
    completed: bool,
}

impl From<todos_entity::Model> for TodoResponse {
    fn from(todo: todos_entity::Model) -> Self {
        Self {
            id: todo.id,
            title: todo.title,
            completed: todo.completed,
        }
    }
}

#[derive(Clone)]
struct TodosController;

impl ResourceController<ApiState> for TodosController {
    type Id = i32;
    type Resource = TodoResponse;
    type Create = CreateTodo;
    type Update = UpdateTodo;

    fn index(&self, state: ApiState) -> ControllerFuture<'_, Vec<Self::Resource>> {
        Box::pin(async move {
            let database = state.database()?;
            let todos = todos_entity::Entity::find()
                .order_by_asc(todos_entity::Column::Id)
                .all(database)
                .await?;

            Ok(todos.into_iter().map(TodoResponse::from).collect())
        })
    }

    fn show(&self, state: ApiState, id: Self::Id) -> ControllerFuture<'_, Option<Self::Resource>> {
        Box::pin(async move {
            let database = state.database()?;
            let todo = todos_entity::Entity::find_by_id(id).one(database).await?;

            Ok(todo.map(TodoResponse::from))
        })
    }

    fn create(&self, state: ApiState, input: Self::Create) -> ControllerFuture<'_, Self::Resource> {
        Box::pin(async move {
            let database = state.database()?;
            let inserted = todos_entity::ActiveModel {
                title: Set(input.title),
                completed: Set(false),
                ..Default::default()
            }
            .insert(database)
            .await?;

            Ok(inserted.into())
        })
    }

    fn update(
        &self,
        state: ApiState,
        id: Self::Id,
        input: Self::Update,
    ) -> ControllerFuture<'_, Option<Self::Resource>> {
        Box::pin(async move {
            let database = state.database()?;
            let Some(todo) = todos_entity::Entity::find_by_id(id).one(database).await? else {
                return Ok(None);
            };

            let mut active: todos_entity::ActiveModel = todo.into();
            if let Some(title) = input.title {
                active.title = Set(title);
            }
            if let Some(completed) = input.completed {
                active.completed = Set(completed);
            }

            let updated = active.update(database).await?;

            Ok(Some(updated.into()))
        })
    }

    fn delete(&self, state: ApiState, id: Self::Id) -> ControllerFuture<'_, bool> {
        Box::pin(async move {
            let database = state.database()?;
            let result = todos_entity::Entity::delete_by_id(id)
                .exec(database)
                .await?;

            Ok(result.rows_affected == 1)
        })
    }

    fn resource_id(&self, resource: &Self::Resource) -> Self::Id {
        resource.id
    }
}

#[tokio::test]
async fn seaorm_sqlite_direct_row_lifecycle() {
    let database = sqlite_database_with_todos_schema().await;

    let inserted = todos_entity::ActiveModel {
        title: Set("write the SeaORM fixture".to_owned()),
        completed: Set(false),
        ..Default::default()
    }
    .insert(&database)
    .await
    .expect("todo row should insert through the SeaORM entity");

    let found = todos_entity::Entity::find_by_id(inserted.id)
        .one(&database)
        .await
        .expect("inserted todo should be readable through the SeaORM entity")
        .expect("inserted todo should exist");

    assert_eq!(found.id, inserted.id);
    assert_eq!(found.title, "write the SeaORM fixture");
    assert!(!found.completed);
}

#[tokio::test]
async fn seaorm_sqlite_state_and_error_mapping_round_trips_row_through_framework_state() {
    async fn create_then_read_todo(State(state): State<ApiState>) -> ApiResult<Json<Value>> {
        let database = state.database()?;
        let inserted = todos_entity::ActiveModel {
            title: Set("created through framework state".to_owned()),
            completed: Set(true),
            ..Default::default()
        }
        .insert(database)
        .await?;

        let found = todos_entity::Entity::find_by_id(inserted.id)
            .one(database)
            .await?
            .ok_or(ApiError::Internal)?;

        Ok(Json(json!({
            "id": found.id,
            "title": found.title,
            "completed": found.completed,
        })))
    }

    let app = Api::new(sqlite_state_with_todos_schema().await)
        .route("/todos", get(create_then_read_todo))
        .into_router();

    let response = call(app, "/todos").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_json_content_type(&response);
    assert_eq!(
        json_body(response).await,
        json!({
            "id": 1,
            "title": "created through framework state",
            "completed": true,
        })
    );
}

#[tokio::test]
async fn controller_crud_successful_sqlite_lifecycle() {
    let app = Api::new(sqlite_state_with_todos_schema().await)
        .resource("/todos", TodosController)
        .into_router();

    let response = call(app.clone(), "/todos").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_content_type(&response);
    assert_eq!(json_body(response).await, json!([]));

    let response = json_request(
        app.clone(),
        Method::POST,
        "/todos",
        json!({ "title": "prove sqlite-backed controller CRUD" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_json_content_type(&response);
    assert_eq!(
        response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/todos/1")
    );
    assert_eq!(
        json_body(response).await,
        json!({
            "id": 1,
            "title": "prove sqlite-backed controller CRUD",
            "completed": false,
        })
    );

    let response = call(app.clone(), "/todos/1").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_content_type(&response);
    assert_eq!(
        json_body(response).await,
        json!({
            "id": 1,
            "title": "prove sqlite-backed controller CRUD",
            "completed": false,
        })
    );

    let response = json_request(
        app.clone(),
        Method::PATCH,
        "/todos/1",
        json!({
            "title": "proved sqlite-backed controller CRUD",
            "completed": true,
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_content_type(&response);
    assert_eq!(
        json_body(response).await,
        json!({
            "id": 1,
            "title": "proved sqlite-backed controller CRUD",
            "completed": true,
        })
    );

    let response = call(app.clone(), "/todos").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_content_type(&response);
    assert_eq!(
        json_body(response).await,
        json!([{
            "id": 1,
            "title": "proved sqlite-backed controller CRUD",
            "completed": true,
        }])
    );

    let response = request(app.clone(), Method::DELETE, "/todos/1", Body::empty()).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(body_bytes(response).await.is_empty());

    let response = call(app, "/todos").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_content_type(&response);
    assert_eq!(json_body(response).await, json!([]));
}

#[tokio::test]
async fn controller_crud_failure_paths_report_missing_resources() {
    let state = sqlite_state_with_todos_schema().await;
    let app = Api::new(state.clone())
        .resource("/todos", TodosController)
        .into_router();

    let response = call(app.clone(), "/todos/999").await;
    assert_not_found_problem_response(response).await;
    assert_eq!(todo_count(&state).await, 0);

    let response = json_request(
        app.clone(),
        Method::PATCH,
        "/todos/999",
        json!({ "title": "missing todo" }),
    )
    .await;
    assert_not_found_problem_response(response).await;
    assert_eq!(todo_count(&state).await, 0);

    let response = request(app, Method::DELETE, "/todos/999", Body::empty()).await;
    assert_not_found_problem_response(response).await;
    assert_eq!(todo_count(&state).await, 0);
}

#[tokio::test]
async fn controller_crud_failure_paths_reject_malformed_inputs_without_mutation() {
    let state = sqlite_state_with_todos_schema().await;
    let app = Api::new(state.clone())
        .resource("/todos", TodosController)
        .into_router();

    let response = json_request(
        app.clone(),
        Method::POST,
        "/todos",
        json!({ "title": "baseline todo" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let before = todo_rows(&state).await;
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].title, "baseline todo");
    assert!(!before[0].completed);

    let response = call(app.clone(), "/todos/not-an-integer").await;
    assert_client_error_response(response).await;
    assert_eq!(todo_rows(&state).await, before);

    let response = request(
        app.clone(),
        Method::POST,
        "/todos",
        Body::from(r#"{"title":42}"#),
    )
    .await;
    assert_client_error_response(response).await;
    assert_eq!(todo_rows(&state).await, before);

    let response = request(
        app,
        Method::PATCH,
        "/todos/1",
        Body::from(r#"{"completed":"not-a-bool"}"#),
    )
    .await;
    assert_client_error_response(response).await;
    assert_eq!(todo_rows(&state).await, before);
}

#[tokio::test]
async fn controller_crud_failure_paths_redact_missing_database_state() {
    let app = Api::new(ApiState::default())
        .resource("/todos", TodosController)
        .into_router();

    let response = call(app, "/todos").await;

    assert_problem_response_redacts(
        response,
        &["database", "framework_state", "SQL", "SELECT", "todos"],
    )
    .await;
}

#[tokio::test]
async fn seaorm_sqlite_state_and_error_mapping_sanitizes_query_failures() {
    async fn missing_table_query(State(state): State<ApiState>) -> ApiResult<Json<Value>> {
        state
            .database()?
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT id FROM definitely_missing_todos".to_owned(),
            ))
            .await?;

        Ok(Json(json!({ "unexpected": true })))
    }

    let app = Api::new(sqlite_state_with_todos_schema().await)
        .route("/failing-query", get(missing_table_query))
        .into_router();

    let response = call(app, "/failing-query").await;

    assert_problem_response_redacts(response, &["SELECT", "definitely_missing_todos"]).await;
}

#[tokio::test]
async fn seaorm_sqlite_state_and_error_mapping_sanitizes_missing_database_state() {
    async fn requires_database(State(state): State<ApiState>) -> ApiResult<Json<Value>> {
        let _database = state.database()?;

        Ok(Json(json!({ "unexpected": true })))
    }

    let app = Api::new(ApiState::default())
        .route("/requires-database", get(requires_database))
        .into_router();

    let response = call(app, "/requires-database").await;

    assert_problem_response_redacts(response, &["database", "framework_state"]).await;
}

async fn call(app: Router, uri: &str) -> Response {
    app.oneshot(
        Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("test request should build"),
    )
    .await
    .expect("service call should succeed")
}

async fn json_request(app: Router, method: Method, uri: &str, body: Value) -> Response {
    request(app, method, uri, Body::from(body.to_string())).await
}

async fn request(app: Router, method: Method, uri: &str, body: Body) -> Response {
    app.oneshot(
        Request::builder()
            .method(method)
            .uri(uri)
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .expect("test request should build"),
    )
    .await
    .expect("service call should succeed")
}

fn assert_json_content_type(response: &Response) {
    assert_eq!(
        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
}

async fn json_body(response: Response) -> Value {
    let (body, _) = json_body_and_text(response).await;
    body
}

async fn json_body_and_text(response: Response) -> (Value, String) {
    let body = body_bytes(response).await;
    let body_text = String::from_utf8(body.to_vec()).expect("response body should be UTF-8 JSON");
    let body = serde_json::from_str(&body_text).expect("response body should be valid JSON");

    (body, body_text)
}

async fn body_bytes(response: Response) -> axum::body::Bytes {
    to_bytes(response.into_body(), 1024)
        .await
        .expect("response body should be readable")
}

async fn todo_rows(state: &ApiState) -> Vec<todos_entity::Model> {
    todos_entity::Entity::find()
        .order_by_asc(todos_entity::Column::Id)
        .all(
            state
                .database()
                .expect("framework state should expose the configured database"),
        )
        .await
        .expect("todo rows should be readable")
}

async fn todo_count(state: &ApiState) -> usize {
    todo_rows(state).await.len()
}

async fn assert_client_error_response(response: Response) {
    let status = response.status();
    assert!(
        status.is_client_error(),
        "expected client error status for rejected input, got {status}"
    );

    let body_text = body_text(response).await;
    assert!(
        !body_text.contains("database") && !body_text.contains("framework_state"),
        "client rejection body should not mention framework internals: {body_text}"
    );
}

async fn assert_not_found_problem_response(response: Response) {
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_json_content_type(&response);

    let (body, body_text) = json_body_and_text(response).await;
    assert_eq!(
        body,
        json!({
            "status": 404,
            "error": "not_found",
            "message": "resource not found",
        })
    );

    for fragment in ["database", "framework_state", "SQL", "SELECT", "todos"] {
        assert!(
            !body_text.contains(fragment),
            "not-found problem body leaked forbidden fragment {fragment:?}: {body_text}"
        );
    }
}

async fn body_text(response: Response) -> String {
    let body = body_bytes(response).await;
    String::from_utf8(body.to_vec()).expect("response body should be UTF-8")
}

async fn assert_problem_response_redacts(response: Response, forbidden_fragments: &[&str]) {
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_json_content_type(&response);

    let (body, body_text) = json_body_and_text(response).await;

    assert_eq!(
        body,
        json!({
            "status": 500,
            "error": "internal_server_error",
            "message": "internal server error",
        })
    );

    for fragment in forbidden_fragments {
        assert!(
            !body_text.contains(fragment),
            "public problem body leaked forbidden fragment {fragment:?}: {body_text}"
        );
    }
}
