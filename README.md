# ruw

`ruw` is a lightweight Rust web-framework facade that keeps Axum close at hand. The initial API is intentionally macro-light: applications assemble normal Axum routes through `Api`, keep typed state until finalization, and can drop down to raw Axum whenever the facade is not the right abstraction.

## Facade sketch

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

let raw_axum_routes = Router::<AppState>::new().route("/health", get(health));

let app = Api::new(state)
    .route("/hello", get(hello))
    .merge(raw_axum_routes)
    .into_router();

// `app` is now a normal `axum::Router` and can be served by Axum/Tower tooling.
# let _ = app;
```

## SeaORM SQLite substrate

`ApiState` can carry a framework-managed SeaORM SQLite connection while remaining defaultable and cloneable. Use `ApiState::connect_sqlite` when the framework should open the connection, or `ApiState::with_database` when the application has already built a SeaORM `DatabaseConnection`.

```rust
use ruw::{
    axum::routing::get,
    prelude::*,
    sea_orm::{ConnectionTrait, DatabaseBackend, Statement},
};
use serde_json::{json, Value};

async fn check_database(State(state): State<ApiState>) -> ApiResult<Json<Value>> {
    let database = state.database()?;

    database
        .query_one(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT 1".to_owned(),
        ))
        .await?;

    Ok(Json(json!({ "database": "ok" })))
}

let state = ApiState::connect_sqlite("sqlite::memory:").await?;
let app = Api::new(state)
    .route("/db-check", get(check_database))
    .into_router();
# let _ = app;
# Ok::<(), ruw::ApiError>(())
```

Handlers and controllers access the stored connection with `state.database()?`; a missing database is reported through the normal `ApiError` seam instead of panicking. The crate also re-exports `ruw::connect_sqlite` for opening a bare connection and `ruw::sea_orm` as an explicit escape hatch for resource-specific SeaORM queries.

Resource code owns its own SeaORM queries. `ruw` provides state plumbing and response conversion, not a generic repository layer or automatic ORM abstraction.

## Controller CRUD routes

Use `ResourceController` when a resource should expose conventional CRUD routes without procedural macros. A controller is an ordinary cloneable Rust type with typed associated types for its path id, public JSON resource, create DTO, and update DTO. Each method returns a boxed `ControllerFuture` so the public API does not require an `async-trait` dependency.

The sketch below uses placeholder query helpers to focus on the public shape; in real code those helpers are ordinary resource-owned SeaORM calls that use `state.database()?`.

```rust
use ruw::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
struct TodosController;

#[derive(Serialize)]
struct TodoResponse {
    id: i32,
    title: String,
    completed: bool,
}

#[derive(Deserialize)]
struct CreateTodo {
    title: String,
}

#[derive(Deserialize)]
struct UpdateTodo {
    title: Option<String>,
    completed: Option<bool>,
}

impl ResourceController<ApiState> for TodosController {
    type Id = i32;
    type Resource = TodoResponse;
    type Create = CreateTodo;
    type Update = UpdateTodo;

    fn index(&self, state: ApiState) -> ControllerFuture<'_, Vec<Self::Resource>> {
        Box::pin(async move { list_todos(state.database()?).await })
    }

    fn show(&self, state: ApiState, id: Self::Id) -> ControllerFuture<'_, Option<Self::Resource>> {
        Box::pin(async move { find_todo(state.database()?, id).await })
    }

    fn create(&self, state: ApiState, input: Self::Create) -> ControllerFuture<'_, Self::Resource> {
        Box::pin(async move { insert_todo(state.database()?, input).await })
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
        Box::pin(async move { update_todo(state.database()?, id, input).await })
    }

    fn validate_update(&self, input: &Self::Update) -> ApiResult<()> {
        if let Some(title) = &input.title {
            validate_title(title)?;
        }

        Ok(())
    }

    fn delete(&self, state: ApiState, id: Self::Id) -> ControllerFuture<'_, bool> {
        Box::pin(async move { delete_todo(state.database()?, id).await })
    }

    fn resource_id(&self, resource: &Self::Resource) -> Self::Id {
        resource.id
    }
}

let state = ApiState::connect_sqlite("sqlite::memory:").await?;
let app = Api::new(state)
    .resource("/todos", TodosController)
    .into_router();
# let _ = app;
# Ok::<(), ruw::ApiError>(())
```

`Api::resource("/todos", TodosController)` registers the collection route and the member route once. Member routes use Axum 0.8 `{id}` path syntax, not the older `:id` syntax.

| Method | Path | Controller method | Success behavior |
|---|---|---|---|
| `GET` | `/todos` | `index` | `200 OK` with a JSON array |
| `POST` | `/todos` | `create` | `201 Created` with a JSON body and `Location: /todos/{id}` |
| `GET` | `/todos/{id}` | `show` | `200 OK` with a JSON body, or a sanitized `404` when missing |
| `PATCH` | `/todos/{id}` | `update` | `200 OK` with a JSON body, or a sanitized `404` when missing |
| `DELETE` | `/todos/{id}` | `delete` | `204 No Content` with an empty body, or a sanitized `404` when missing |

`validate_create` and `validate_update` are optional synchronous hooks. They run after framework-owned JSON/path extraction succeeds and before `create` or `update` mutates state; returning `ApiError::ValidationFailed` renders the stable `422` problem body without calling the mutation method.

Typed ids and JSON bodies for `Api::resource` routes are parsed through framework-owned extractors before controller code runs. Malformed member ids, malformed JSON syntax, wrong JSON value types, and missing JSON content types map to the stable `400 bad_request` problem response and stop before resource mutation. Raw Axum routes registered with `Api::route` or `Api::merge` remain an escape hatch: they keep Axum's normal extractor behavior unless the application deliberately uses framework helpers in those routes.

## Public JSON problem contract

Framework-owned errors render a stable JSON `Problem` body with exactly these public fields:

```json
{
  "status": 400,
  "error": "bad_request",
  "message": "bad request"
}
```

The `status` field is the numeric HTTP status, `error` is the stable machine-readable code, and `message` is a sanitized human-readable string. `Api::resource` currently commits to the following public matrix:

| HTTP status | `error` | `message` | Source |
|---|---|---|---|
| `400 Bad Request` | `bad_request` | `bad request` | Framework-owned path and JSON extractor failures for resource routes. |
| `422 Unprocessable Entity` | `validation_failed` | `validation failed` | `ResourceController::validate_create` or `validate_update` rejects a typed DTO before mutation. |
| `404 Not Found` | `not_found` | `resource not found` | Controller `show`, `update`, or `delete` reports that the requested resource is missing. |
| `500 Internal Server Error` | `internal_server_error` | `internal server error` | SeaORM/database failures, missing framework database state, invalid framework-generated headers, or other internal framework/application failures. |

The framework deliberately does not echo path fragments, raw request bodies, validation input values, SQL statements, SQLite paths, connection strings, table names, raw SeaORM messages, or framework-state labels in public problem bodies. Those details are diagnostics-only: `tracing` records structured categories such as `error_kind = "bad_request"`, `"validation_failed"`, `"not_found"`, `"database"`, or `"framework_state"` so future agents can inspect internals without changing the client-visible contract.

## Running the todos API example

The live todos example wires `Api::resource`, `ResourceController<ApiState>`, SeaORM SQLite, and the public JSON problem contract into a real Axum process. It is local-only, unauthenticated, SQLite-only, and intentionally does not add code generators, auth, migrations, or production lifecycle management.

By default the example listens on `127.0.0.1:3000` and stores data in `target/todos_api/todos.sqlite`:

```sh
cargo run --example todos_api
```

The startup log prints the actual bound URL as `todos_api listening on http://...`. Override both runtime inputs when you want an isolated port and database file:

```sh
RUW_TODOS_ADDR=127.0.0.1:0 \
RUW_TODOS_DATABASE_URL="sqlite://target/todos_api/dev.sqlite?mode=rwc" \
cargo run --example todos_api
```

Use the following endpoints against the printed base URL:

| Method | Path | Request body | Success behavior |
|---|---|---|---|
| `GET` | `/health` | none | `200 OK` with `{"status":"ok"}` after a lightweight database check. |
| `GET` | `/todos` | none | `200 OK` with a JSON array ordered by id. |
| `POST` | `/todos` | `{"title":"ship docs"}` | `201 Created` with the created todo, `completed: false`, and `Location: /todos/{id}`. |
| `GET` | `/todos/{id}` | none | `200 OK` with one todo, or the sanitized `404` problem body when missing. |
| `PATCH` | `/todos/{id}` | `{"title":"ship verified docs","completed":true}` | `200 OK` with the updated todo, or the sanitized `404` problem body when missing. |
| `DELETE` | `/todos/{id}` | none | `204 No Content` with an empty body, or the sanitized `404` problem body when missing. |

Malformed member ids, malformed JSON, wrong JSON types, and missing JSON content types are framework-owned extractor failures and return the canonical `400 bad_request` problem response. Blank create or update titles are rejected by the controller validation hooks before mutation and return `422 validation_failed`. Database or other internal failures return the sanitized `500 internal_server_error` problem response; public bodies do not expose SQL, SQLite paths, connection strings, table names, or raw SeaORM diagnostics.

For final local verification, run:

```sh
python3 scripts/verify_todos_api.py
```

The verifier starts `cargo run --example todos_api` as a child process with `RUW_TODOS_ADDR=127.0.0.1:0` and an isolated temporary SQLite database, waits for `/health`, then exercises create, list, fetch, update, delete, representative `400`, `404`, `422`, and sanitized `500` behavior over real HTTP. Successful runs clean up their temporary directory; failures print and retain the child-process log path so startup, bind, schema, HTTP, and response-contract issues can be inspected later.

## Public surface in this slice

- `Api<S>` wraps `axum::Router<S>` while routes are assembled.
- `Api::route` accepts normal Axum `MethodRouter<S>` values.
- `Api::merge` accepts raw `axum::Router<S>` values as an escape hatch.
- `Api::resource` expands one `ResourceController` into conventional collection and member CRUD routes.
- `Api::into_router` applies the owned state with Axum's `with_state` and returns a standard `axum::Router`.
- `ApiState` is the default cloneable framework state placeholder and can carry a SeaORM SQLite `DatabaseConnection`.
- `ApiState::connect_sqlite`, `ApiState::with_database`, and `ApiState::database` provide the explicit framework-managed database path.
- `ResourceController` and `ControllerFuture` define the macro-light controller adapter contract.
- `ruw::connect_sqlite` opens a SQLite SeaORM connection outside state when needed.
- `ruw::sea_orm` re-exports SeaORM for explicit resource-specific queries.
- `ApiResult`, `ApiError`, and `Problem` define the framework error seam. Framework-owned resource extractor failures, validation failures, not-found results, internal errors, missing database state, and database errors produce stable sanitized JSON while tracing diagnostics internally.
- `ruw::prelude` exports the common facade, controller, error, state, and handler types, and `ruw::axum` explicitly re-exports Axum for direct interop.

## Verification surfaces

- `cargo test controller_routes` proves the public `ResourceController` trait and `Api::resource` adapter register collection and member routes with typed path and JSON extraction.
- `cargo test controller_crud` proves a SQLite-backed todos controller can create, list, fetch, update, delete, return `Location`, return `204 No Content`, reject malformed inputs without mutation, and preserve sanitized problem responses.
- `cargo test json_error_contract` proves malformed framework-owned resource path and JSON extractor failures return canonical `400 bad_request` JSON and do not call or mutate the controller.
- `cargo test json_response_contract` proves the resource-route response matrix for `200`, `201` with `Location`, `204` empty delete, `400`, `404`, `422`, and sanitized `500` through Axum/Tower service tests against isolated in-memory SQLite.
- `cargo test seaorm_sqlite` is the focused database-substrate and redaction check.
- `cargo check --example todos_api` proves the runnable example compiles against the public facade and SQLite dependencies.
- `python3 scripts/verify_todos_api.py` proves the live `cargo run --example todos_api` process over localhost TCP with an isolated SQLite file.
- Slice closeout uses `cargo check --example todos_api && cargo fmt --check && cargo test && python3 scripts/verify_todos_api.py` to re-verify the facade, SeaORM substrate, controller adapter, JSON contract, error redaction behavior, and live example together.

## Current boundary

This milestone currently proves the Axum facade, typed state flow, raw-route merge path, SeaORM SQLite substrate, reusable Todos entity fixture, controller CRUD registration, SQLite-backed controller lifecycle, stable JSON success/error contract through service tests, and a live HTTP `todos_api` process through network-level verification. Framework-owned resource routes now have documented public behavior for successful CRUD, malformed extractor input, semantic validation failures, missing resources, and sanitized internal/database failures. The shipped scope remains a local unauthenticated API facade and SQLite example; auth, generators, migrations, non-SQLite databases, and production lifecycle management are outside M001.
