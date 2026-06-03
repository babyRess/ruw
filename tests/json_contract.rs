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
        Router,
    },
    prelude::*,
    sea_orm::{
        ActiveModelTrait, ConnectionTrait, DatabaseBackend, EntityTrait, QueryOrder, Set, Statement,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct ContractTodo {
    id: u64,
    title: String,
    completed: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct ContractCreateTodo {
    title: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ContractUpdateTodo {
    title: Option<String>,
    completed: Option<bool>,
}

#[derive(Clone, Default)]
struct ContractState {
    store: Arc<Mutex<ContractStore>>,
}

#[derive(Default)]
struct ContractStore {
    next_id: u64,
    todos: Vec<ContractTodo>,
    controller_calls: usize,
    mutations: usize,
}

#[derive(Clone)]
struct ContractTodos;

impl ResourceController<ContractState> for ContractTodos {
    type Id = u64;
    type Resource = ContractTodo;
    type Create = ContractCreateTodo;
    type Update = ContractUpdateTodo;

    fn index(&self, state: ContractState) -> ControllerFuture<'_, Vec<Self::Resource>> {
        Box::pin(async move {
            let mut store = state
                .store
                .lock()
                .expect("store mutex should not be poisoned");
            store.controller_calls += 1;

            Ok(store.todos.clone())
        })
    }

    fn show(
        &self,
        state: ContractState,
        id: Self::Id,
    ) -> ControllerFuture<'_, Option<Self::Resource>> {
        Box::pin(async move {
            let mut store = state
                .store
                .lock()
                .expect("store mutex should not be poisoned");
            store.controller_calls += 1;

            Ok(store.todos.iter().find(|todo| todo.id == id).cloned())
        })
    }

    fn create(
        &self,
        state: ContractState,
        input: Self::Create,
    ) -> ControllerFuture<'_, Self::Resource> {
        Box::pin(async move {
            let mut store = state
                .store
                .lock()
                .expect("store mutex should not be poisoned");
            store.controller_calls += 1;
            store.next_id += 1;
            store.mutations += 1;

            let todo = ContractTodo {
                id: store.next_id,
                title: input.title,
                completed: false,
            };
            store.todos.push(todo.clone());

            Ok(todo)
        })
    }

    fn update(
        &self,
        state: ContractState,
        id: Self::Id,
        input: Self::Update,
    ) -> ControllerFuture<'_, Option<Self::Resource>> {
        Box::pin(async move {
            let mut store = state
                .store
                .lock()
                .expect("store mutex should not be poisoned");
            store.controller_calls += 1;

            let Some(position) = store.todos.iter().position(|todo| todo.id == id) else {
                return Ok(None);
            };

            if let Some(title) = input.title {
                store.todos[position].title = title;
            }
            if let Some(completed) = input.completed {
                store.todos[position].completed = completed;
            }
            let updated = store.todos[position].clone();
            store.mutations += 1;

            Ok(Some(updated))
        })
    }

    fn delete(&self, state: ContractState, id: Self::Id) -> ControllerFuture<'_, bool> {
        Box::pin(async move {
            let mut store = state
                .store
                .lock()
                .expect("store mutex should not be poisoned");
            store.controller_calls += 1;

            let Some(position) = store.todos.iter().position(|todo| todo.id == id) else {
                return Ok(false);
            };

            store.todos.remove(position);
            store.mutations += 1;

            Ok(true)
        })
    }

    fn resource_id(&self, resource: &Self::Resource) -> Self::Id {
        resource.id
    }
}

#[derive(Clone, Debug, Deserialize)]
struct SqliteCreateTodo {
    title: String,
}

#[derive(Clone, Debug, Deserialize)]
struct SqliteUpdateTodo {
    title: Option<String>,
    completed: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
struct SqliteTodoResponse {
    id: i32,
    title: String,
    completed: bool,
}

impl From<todos_entity::Model> for SqliteTodoResponse {
    fn from(todo: todos_entity::Model) -> Self {
        Self {
            id: todo.id,
            title: todo.title,
            completed: todo.completed,
        }
    }
}

#[derive(Clone)]
struct SqliteTodosController;

impl ResourceController<ApiState> for SqliteTodosController {
    type Id = i32;
    type Resource = SqliteTodoResponse;
    type Create = SqliteCreateTodo;
    type Update = SqliteUpdateTodo;

    fn index(&self, state: ApiState) -> ControllerFuture<'_, Vec<Self::Resource>> {
        Box::pin(async move {
            let database = state.database()?;
            let todos = todos_entity::Entity::find()
                .order_by_asc(todos_entity::Column::Id)
                .all(database)
                .await?;

            Ok(todos.into_iter().map(SqliteTodoResponse::from).collect())
        })
    }

    fn show(&self, state: ApiState, id: Self::Id) -> ControllerFuture<'_, Option<Self::Resource>> {
        Box::pin(async move {
            let database = state.database()?;
            let todo = todos_entity::Entity::find_by_id(id).one(database).await?;

            Ok(todo.map(SqliteTodoResponse::from))
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

    fn validate_create(&self, input: &Self::Create) -> ApiResult<()> {
        validate_title(&input.title)
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

    fn validate_update(&self, input: &Self::Update) -> ApiResult<()> {
        if let Some(title) = &input.title {
            validate_title(title)?;
        }

        Ok(())
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

fn validate_title(title: &str) -> ApiResult<()> {
    if title.trim().is_empty() {
        Err(ApiError::ValidationFailed)
    } else {
        Ok(())
    }
}

#[tokio::test]
async fn json_error_contract_malformed_member_id_returns_canonical_400_without_mutation() {
    let state = ContractState::default();
    let app = contract_app(state.clone());

    let response = request(
        app,
        Method::PATCH,
        "/todos/not-a-number",
        Body::from(json!({ "title": "should not be applied" }).to_string()),
    )
    .await;

    let body_text = assert_bad_request_problem(response).await;
    assert!(!body_text.contains("not-a-number"));
    assert_no_controller_call_or_mutation(&state);
}

#[tokio::test]
async fn json_error_contract_malformed_json_syntax_returns_canonical_400_without_mutation() {
    let state = ContractState::default();
    let app = contract_app(state.clone());

    let response = request(app, Method::POST, "/todos", Body::from("{ not valid json")).await;

    let body_text = assert_bad_request_problem(response).await;
    assert!(!body_text.contains("not valid json"));
    assert_no_controller_call_or_mutation(&state);
}

#[tokio::test]
async fn json_error_contract_wrong_json_value_type_returns_canonical_400_without_mutation() {
    let state = ContractState::default();
    let app = contract_app(state.clone());

    let response = request(
        app,
        Method::PATCH,
        "/todos/1",
        Body::from(json!({ "completed": "not a boolean" }).to_string()),
    )
    .await;

    let body_text = assert_bad_request_problem(response).await;
    assert!(!body_text.contains("not a boolean"));
    assert_no_controller_call_or_mutation(&state);
}

#[tokio::test]
async fn json_response_contract_sqlite_crud_success_matrix() {
    let state = sqlite_state_with_todos_schema().await;
    let app = sqlite_contract_app(state);

    let response = call(app.clone(), "/todos").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_content_type(&response);
    assert_eq!(json_body(response).await, json!([]));

    let response = json_request(
        app.clone(),
        Method::POST,
        "/todos",
        json!({ "title": "write the contract" }),
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
            "title": "write the contract",
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
            "title": "write the contract",
            "completed": false,
        })
    );

    let response = json_request(
        app.clone(),
        Method::PATCH,
        "/todos/1",
        json!({
            "title": "completed the contract",
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
            "title": "completed the contract",
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
            "title": "completed the contract",
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
async fn json_response_contract_malformed_extractors_return_400_without_sqlite_mutation() {
    let state = sqlite_state_with_todos_schema().await;
    let app = sqlite_contract_app(state.clone());

    let response = json_request(
        app.clone(),
        Method::POST,
        "/todos",
        json!({ "title": "baseline" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let before = todo_rows(&state).await;
    assert_eq!(before.len(), 1);

    let response = call(app.clone(), "/todos/not-an-integer").await;
    let body_text = assert_bad_request_problem(response).await;
    assert!(!body_text.contains("not-an-integer"));
    assert_eq!(todo_rows(&state).await, before);

    let response = request(
        app.clone(),
        Method::POST,
        "/todos",
        Body::from("{ not valid json"),
    )
    .await;
    let body_text = assert_bad_request_problem(response).await;
    assert!(!body_text.contains("not valid json"));
    assert_eq!(todo_rows(&state).await, before);

    let response = request(
        app,
        Method::PATCH,
        "/todos/1",
        Body::from(r#"{"completed":"not-a-bool"}"#),
    )
    .await;
    let body_text = assert_bad_request_problem(response).await;
    assert!(!body_text.contains("not-a-bool"));
    assert_eq!(todo_rows(&state).await, before);
}

#[tokio::test]
async fn json_response_contract_missing_resources_return_404_without_sqlite_mutation() {
    let state = sqlite_state_with_todos_schema().await;
    let app = sqlite_contract_app(state.clone());

    let response = call(app.clone(), "/todos/999").await;
    assert_problem_response(
        response,
        StatusCode::NOT_FOUND,
        json!({
            "status": 404,
            "error": "not_found",
            "message": "resource not found",
        }),
    )
    .await;
    assert_eq!(todo_count(&state).await, 0);

    let response = json_request(
        app.clone(),
        Method::PATCH,
        "/todos/999",
        json!({ "title": "still missing" }),
    )
    .await;
    assert_problem_response(
        response,
        StatusCode::NOT_FOUND,
        json!({
            "status": 404,
            "error": "not_found",
            "message": "resource not found",
        }),
    )
    .await;
    assert_eq!(todo_count(&state).await, 0);

    let response = request(app, Method::DELETE, "/todos/999", Body::empty()).await;
    assert_problem_response(
        response,
        StatusCode::NOT_FOUND,
        json!({
            "status": 404,
            "error": "not_found",
            "message": "resource not found",
        }),
    )
    .await;
    assert_eq!(todo_count(&state).await, 0);
}

#[tokio::test]
async fn json_response_contract_validation_failures_return_422_without_sqlite_mutation() {
    let state = sqlite_state_with_todos_schema().await;
    let app = sqlite_contract_app(state.clone());

    let response = json_request(
        app.clone(),
        Method::POST,
        "/todos",
        json!({ "title": "   " }),
    )
    .await;
    let body_text = assert_problem_response(
        response,
        StatusCode::UNPROCESSABLE_ENTITY,
        json!({
            "status": 422,
            "error": "validation_failed",
            "message": "validation failed",
        }),
    )
    .await;
    assert!(!body_text.contains("   "));
    assert_eq!(todo_count(&state).await, 0);

    let response = json_request(
        app.clone(),
        Method::POST,
        "/todos",
        json!({ "title": "baseline" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let before = todo_rows(&state).await;
    assert_eq!(before.len(), 1);

    let response = json_request(app, Method::PATCH, "/todos/1", json!({ "title": "" })).await;
    assert_problem_response(
        response,
        StatusCode::UNPROCESSABLE_ENTITY,
        json!({
            "status": 422,
            "error": "validation_failed",
            "message": "validation failed",
        }),
    )
    .await;
    assert_eq!(todo_rows(&state).await, before);
}

#[tokio::test]
async fn json_response_contract_missing_database_state_returns_sanitized_500() {
    let app = sqlite_contract_app(ApiState::default());

    let response = call(app, "/todos").await;

    assert_internal_problem_redacts(
        response,
        &[
            "SQL",
            "SELECT",
            "todos",
            "database",
            "framework_state",
            "sqlite",
            "SQLite",
            "SeaORM",
        ],
    )
    .await;
}

#[tokio::test]
async fn json_response_contract_missing_table_query_failure_returns_sanitized_500() {
    let state = ApiState::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite database should connect");
    let app = sqlite_contract_app(state);

    let response = call(app, "/todos").await;

    assert_internal_problem_redacts(
        response,
        &[
            "SQL",
            "SELECT",
            "todos",
            "database",
            "framework_state",
            "sqlite",
            "SQLite",
            "SeaORM",
            "no such table",
        ],
    )
    .await;
}

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

fn sqlite_contract_app(state: ApiState) -> Router {
    Api::new(state)
        .resource("/todos", SqliteTodosController)
        .into_router()
}

fn contract_app(state: ContractState) -> Router {
    Api::new(state)
        .resource("/todos", ContractTodos)
        .into_router()
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

async fn call(app: Router, uri: &str) -> Response {
    request(app, Method::GET, uri, Body::empty()).await
}

async fn json_request(app: Router, method: Method, uri: &str, body: Value) -> Response {
    request(app, method, uri, Body::from(body.to_string())).await
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

async fn assert_problem_response(
    response: Response,
    status: StatusCode,
    expected: Value,
) -> String {
    assert_eq!(response.status(), status);
    assert_json_content_type(&response);

    let (body, body_text) = json_body_and_text(response).await;
    assert_eq!(body, expected);

    body_text
}

async fn assert_internal_problem_redacts(
    response: Response,
    forbidden_fragments: &[&str],
) -> String {
    let body_text = assert_problem_response(
        response,
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({
            "status": 500,
            "error": "internal_server_error",
            "message": "internal server error",
        }),
    )
    .await;

    for fragment in forbidden_fragments {
        assert!(
            !body_text.contains(fragment),
            "public problem body leaked forbidden fragment {fragment:?}: {body_text}"
        );
    }

    body_text
}

async fn assert_bad_request_problem(response: Response) -> String {
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );

    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("response body should be readable");
    let body_text = String::from_utf8(body.to_vec()).expect("response body should be UTF-8");
    let body: Value = serde_json::from_str(&body_text).expect("response body should be JSON");

    assert_eq!(
        body,
        json!({
            "status": 400,
            "error": "bad_request",
            "message": "bad request",
        })
    );

    body_text
}

fn assert_no_controller_call_or_mutation(state: &ContractState) {
    let store = state
        .store
        .lock()
        .expect("store mutex should not be poisoned");

    assert_eq!(store.controller_calls, 0);
    assert_eq!(store.mutations, 0);
    assert!(store.todos.is_empty());
}
