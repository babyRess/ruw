# ruw

`ruw` is a small Rust API framework facade over Axum. It keeps the useful parts of Axum visible while adding a few project-level conveniences:

- `Api` for assembling typed-state routes and serving them.
- `ResourceController` for conventional JSON CRUD routes.
- `ReadOnlyResourceController` for collection/member read routes.
- `ApiState` for optional framework-managed SeaORM SQLite state.
- `ruw new` and `ruw generate resource` for quick project scaffolding.

The framework is intentionally macro-light. You write ordinary Rust structs, DTOs, handlers, and controller methods.

## Install From Source

```sh
git clone https://github.com/babyRess/ruw.git
cd ruw
cargo install --path .
```

Or run the generator without installing it:

```sh
cargo run --bin ruw -- --help
```

## Create An API

Create a new app:

```sh
ruw new ./todo-api
cd ./todo-api
cargo run
```

The generated app listens on `127.0.0.1:3000` unless `RUW_ADDR` is set:

```sh
RUW_ADDR=127.0.0.1:4000 cargo run
```

Check the health route:

```sh
curl http://127.0.0.1:3000/health
```

The generated `Cargo.toml` points at the local `ruw` checkout with a path dependency. If you move the app folder, update that dependency path.

## Generate A Resource

From the `ruw` checkout, generate a resource module into an app:

```sh
ruw generate resource Todo --path ./todo-api title:string completed:boolean
```

Supported field types:

| CLI type | Rust type |
|---|---|
| `string`, `str` | `String` |
| `bool`, `boolean` | `bool` |
| `int`, `integer`, `i32`, `i64`, `u32`, `u64` | `i64` |
| `float`, `f32`, `f64` | `f64` |
| `uuid` | `String` |

The generator writes a non-overwriting `src/todo.rs` with:

- `TodoState`
- `TodoController`
- `TodoResponse`
- `CreateTodo`
- `UpdateTodo`
- `api(state)` helper

Wire the generated resource in `src/main.rs`:

```rust
mod todo;

use ruw::{
    axum::{routing::get, Json},
    prelude::*,
};
use serde::Serialize;
use std::net::SocketAddr;

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr: SocketAddr = std::env::var("RUW_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:3000".to_owned())
        .parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    todo::api(todo::TodoState::default())
        .route("/health", get(health))
        .serve(listener)
        .await?;

    Ok(())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}
```

Run the app:

```sh
cargo run
```

Then call the generated JSON routes:

```sh
curl http://127.0.0.1:3000/todos

curl -X POST http://127.0.0.1:3000/todos \
  -H 'Content-Type: application/json' \
  -d '{"title":"ship ruw","completed":false}'

curl -X PATCH http://127.0.0.1:3000/todos/1 \
  -H 'Content-Type: application/json' \
  -d '{"completed":true}'

curl -X DELETE http://127.0.0.1:3000/todos/1
```

## CRUD Route Contract

`Api::resource("/todos", TodoController)` registers:

| Method | Path | Controller method | Success response |
|---|---|---|---|
| `GET` | `/todos` | `index` | `200 OK` JSON array |
| `POST` | `/todos` | `create` | `201 Created` JSON body plus `Location: /todos/{id}` |
| `GET` | `/todos/{id}` | `show` | `200 OK` JSON body |
| `PATCH` | `/todos/{id}` | `update` | `200 OK` JSON body |
| `DELETE` | `/todos/{id}` | `delete` | `204 No Content` |

Missing resources return the framework `404` problem body. Invalid path ids, malformed JSON, wrong JSON types, and missing JSON content types return the framework `400` problem body before controller mutation code runs.

Paths are normalized. These calls mount the same routes:

```rust
Api::new(state).resource("todos", TodoController);
Api::new(state).resource("/todos", TodoController);
Api::new(state).resource("/todos/", TodoController);
```

Use `ReadOnlyResourceController` and `Api::read_only_resource` when a domain only needs `GET /resources` and `GET /resources/{id}`.

## Minimal Axum Interop

Use `Api::route` for normal Axum routes and `Api::merge` when you already have an Axum `Router`.

```rust
use ruw::axum::{routing::get, Router};
use ruw::prelude::*;
use serde::Serialize;

#[derive(Clone)]
struct AppState {
    greeting: String,
}

#[derive(Serialize)]
struct Greeting {
    message: String,
}

async fn hello(State(state): State<AppState>) -> Json<Greeting> {
    Json(Greeting {
        message: state.greeting.clone(),
    })
}

async fn health() -> &'static str {
    "ok"
}

let state = AppState {
    greeting: "hello from ruw".to_owned(),
};

let axum_routes = Router::<AppState>::new().route("/health", get(health));

let app = Api::new(state)
    .route("/hello", get(hello))
    .merge(axum_routes)
    .into_router();

# let _ = app;
```

`into_router()` returns a normal `axum::Router`, so existing Axum and Tower tooling still works.

## Optional SQLite State

`ApiState` can store a SeaORM SQLite connection:

```rust
use ruw::{
    axum::routing::get,
    prelude::*,
    sea_orm::{ConnectionTrait, DatabaseBackend, Statement},
};
use serde_json::{json, Value};

async fn db_check(State(state): State<ApiState>) -> ApiResult<Json<Value>> {
    let database = state.database()?;

    database
        .query_one(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT 1".to_owned(),
        ))
        .await?;

    Ok(Json(json!({ "database": "ok" })))
}

let database_url = std::env::var("DATABASE_URL").ok();
let state = ApiState::connect_optional_sqlite(database_url.as_deref()).await?;

let app = Api::new(state)
    .route("/db-check", get(db_check))
    .into_router();

# let _ = app;
# Ok::<(), ruw::ApiError>(())
```

Use the state helpers based on how strict the app should be:

| Helper | Behavior |
|---|---|
| `ApiState::default()` | no database configured |
| `ApiState::connect_sqlite(url).await?` | database is required |
| `ApiState::connect_optional_sqlite(url).await?` | `None` or blank URL means no database |
| `ApiState::with_database(database)` | use an existing SeaORM connection |

Handlers and controllers call `state.database()?`. If no database is configured, `ruw` returns a sanitized framework error instead of panicking.

## JSON Problem Responses

Framework-owned errors render stable JSON:

```json
{
  "status": 400,
  "error": "bad_request",
  "message": "bad request"
}
```

| HTTP status | `error` | `message` | When it happens |
|---|---|---|---|
| `400 Bad Request` | `bad_request` | `bad request` | Resource path or JSON extractor failure |
| `422 Unprocessable Entity` | `validation_failed` | `validation failed` | Controller validation hook rejects input |
| `404 Not Found` | `not_found` | `resource not found` | Controller reports a missing resource |
| `500 Internal Server Error` | `internal_server_error` | `internal server error` | Internal framework, state, or database failure |

Public problem bodies do not expose path fragments, raw request bodies, SQL, SQLite paths, connection strings, table names, or raw SeaORM messages. Diagnostic details stay in tracing fields.

## Real Todo App

The standalone todo app is in [babyRess/ruw-todo-list](https://github.com/babyRess/ruw-todo-list). It contains:

- Rust API using `ruw`
- Svelte frontend
- Create, edit, complete, delete, priority, description, and smart due date UI

Use that repository when you want a full app. Use this repository when you want the reusable framework and generator.

## Verify This Repository

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Useful targeted checks:

```sh
cargo test cli_run
cargo test controller_routes
cargo test json_response_contract
cargo test seaorm_sqlite
```

## Public Surface

- `Api<S>` wraps `axum::Router<S>` while routes are assembled.
- `Api::route` accepts normal Axum `MethodRouter<S>` values.
- `Api::merge` accepts raw `axum::Router<S>` values.
- `Api::resource` mounts CRUD collection/member routes.
- `Api::read_only_resource` mounts read-only collection/member routes.
- `Api::serve` serves the built API from a `TcpListener`.
- `Api::into_router` returns a normal `axum::Router`.
- `ApiState` carries optional framework-managed SQLite state.
- `ResourceController`, `ReadOnlyResourceController`, and `ControllerFuture` define controller contracts.
- `ApiResult`, `ApiError`, and `Problem` define the framework error contract.
- `ruw::prelude` exports the common facade, controller, error, state, and handler types.
- `ruw::axum` and `ruw::sea_orm` re-export Axum and SeaORM for direct interop.

## Current Boundary

`ruw` currently focuses on local unauthenticated API scaffolding, Axum route assembly, typed controller contracts, SQLite state plumbing, and stable JSON error responses. Auth, migrations, non-SQLite databases, deployment lifecycle, and production persistence policy are left to application code for now.
