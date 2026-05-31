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
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[derive(Clone)]
struct TestState {
    message: &'static str,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct ControllerRouteTodo {
    id: u64,
    title: String,
    completed: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct ControllerRouteCreateTodo {
    title: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ControllerRouteUpdateTodo {
    title: Option<String>,
    completed: Option<bool>,
}

#[derive(Clone, Default)]
struct ControllerRouteState {
    store: Arc<Mutex<ControllerRouteStore>>,
}

#[derive(Default)]
struct ControllerRouteStore {
    next_id: u64,
    todos: Vec<ControllerRouteTodo>,
    mutations: usize,
}

#[derive(Clone)]
struct ControllerRouteTodos;

impl ResourceController<ControllerRouteState> for ControllerRouteTodos {
    type Id = u64;
    type Resource = ControllerRouteTodo;
    type Create = ControllerRouteCreateTodo;
    type Update = ControllerRouteUpdateTodo;

    fn index(&self, state: ControllerRouteState) -> ControllerFuture<'_, Vec<Self::Resource>> {
        Box::pin(async move {
            let store = state
                .store
                .lock()
                .expect("store mutex should not be poisoned");
            Ok(store.todos.clone())
        })
    }

    fn show(
        &self,
        state: ControllerRouteState,
        id: Self::Id,
    ) -> ControllerFuture<'_, Option<Self::Resource>> {
        Box::pin(async move {
            let store = state
                .store
                .lock()
                .expect("store mutex should not be poisoned");
            Ok(store.todos.iter().find(|todo| todo.id == id).cloned())
        })
    }

    fn create(
        &self,
        state: ControllerRouteState,
        input: Self::Create,
    ) -> ControllerFuture<'_, Self::Resource> {
        Box::pin(async move {
            let mut store = state
                .store
                .lock()
                .expect("store mutex should not be poisoned");
            store.next_id += 1;
            store.mutations += 1;

            let todo = ControllerRouteTodo {
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
        state: ControllerRouteState,
        id: Self::Id,
        input: Self::Update,
    ) -> ControllerFuture<'_, Option<Self::Resource>> {
        Box::pin(async move {
            let mut store = state
                .store
                .lock()
                .expect("store mutex should not be poisoned");
            let Some(todo) = store.todos.iter_mut().find(|todo| todo.id == id) else {
                return Ok(None);
            };

            if let Some(title) = input.title {
                todo.title = title;
            }
            if let Some(completed) = input.completed {
                todo.completed = completed;
            }
            let updated = todo.clone();
            store.mutations += 1;

            Ok(Some(updated))
        })
    }

    fn delete(&self, state: ControllerRouteState, id: Self::Id) -> ControllerFuture<'_, bool> {
        Box::pin(async move {
            let mut store = state
                .store
                .lock()
                .expect("store mutex should not be poisoned");
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

#[tokio::test]
async fn stateful_route_returns_json() {
    async fn stateful_handler(State(state): State<TestState>) -> Json<Value> {
        Json(json!({ "message": state.message }))
    }

    let app = Api::new(TestState {
        message: "hello from state",
    })
    .route("/state", get(stateful_handler))
    .into_router();

    let response = call(app, "/state").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_json_content_type(&response);
    assert_eq!(
        json_body(response).await,
        json!({ "message": "hello from state" })
    );
}

#[tokio::test]
async fn raw_axum_router_can_be_merged() {
    async fn raw_handler(State(state): State<TestState>) -> Json<Value> {
        Json(json!({
            "source": "raw_axum_router",
            "message": state.message,
        }))
    }

    let raw_router = Router::<TestState>::new().route("/raw", get(raw_handler));
    let app = Api::new(TestState {
        message: "merged state",
    })
    .merge(raw_router)
    .into_router();

    let response = call(app, "/raw").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_json_content_type(&response);
    assert_eq!(
        json_body(response).await,
        json!({
            "source": "raw_axum_router",
            "message": "merged state",
        })
    );
}

#[tokio::test]
async fn framework_error_returns_problem_json() {
    async fn failing_handler() -> ApiResult<Json<Value>> {
        Err(ApiError::Internal)
    }

    let app = Api::new(TestState {
        message: "should not leak",
    })
    .route("/error", get(failing_handler))
    .into_router();

    let response = call(app, "/error").await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_json_content_type(&response);
    assert_eq!(
        json_body(response).await,
        json!({
            "status": 500,
            "error": "internal_server_error",
            "message": "internal server error",
        })
    );
}

#[tokio::test]
async fn controller_routes_register_collection_and_member_crud_routes() {
    let state = ControllerRouteState::default();
    let app = controller_routes_app(state);

    let response = call(app.clone(), "/todos").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_content_type(&response);
    assert_eq!(json_body(response).await, json!([]));

    let response = json_request(
        app.clone(),
        Method::POST,
        "/todos",
        json!({ "title": "define route adapter" }),
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
        json!({ "id": 1, "title": "define route adapter", "completed": false })
    );

    let response = call(app.clone(), "/todos/1").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_content_type(&response);
    assert_eq!(
        json_body(response).await,
        json!({ "id": 1, "title": "define route adapter", "completed": false })
    );

    let response = json_request(
        app.clone(),
        Method::PATCH,
        "/todos/1",
        json!({ "completed": true }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_content_type(&response);
    assert_eq!(
        json_body(response).await,
        json!({ "id": 1, "title": "define route adapter", "completed": true })
    );

    let response = request(app.clone(), Method::DELETE, "/todos/1", Body::empty()).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(body_bytes(response).await.is_empty());

    let response = call(app, "/todos/1").await;
    assert!(!response.status().is_success());
}

#[tokio::test]
async fn controller_routes_reject_invalid_ids_and_json_without_mutation() {
    let state = ControllerRouteState::default();
    let app = controller_routes_app(state.clone());

    let response = request(
        app.clone(),
        Method::POST,
        "/todos",
        Body::from("{ not valid json"),
    )
    .await;
    assert!(!response.status().is_success());

    let response = call(app, "/todos/not-a-number").await;
    assert!(!response.status().is_success());

    let store = state
        .store
        .lock()
        .expect("store mutex should not be poisoned");
    assert_eq!(store.mutations, 0);
    assert!(store.todos.is_empty());
}

fn controller_routes_app(state: ControllerRouteState) -> Router {
    Api::new(state)
        .resource("/todos", ControllerRouteTodos)
        .into_router()
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
    let body = body_bytes(response).await;

    serde_json::from_slice(&body).expect("response body should be valid JSON")
}

async fn body_bytes(response: Response) -> axum::body::Bytes {
    to_bytes(response.into_body(), 1024)
        .await
        .expect("response body should be readable")
}
