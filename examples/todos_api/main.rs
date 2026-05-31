mod entity;

use std::{env, error::Error, fs, net::SocketAddr, path::Path};

use ruw::{
    axum::{extract::State, routing::get, Json},
    prelude::*,
    sea_orm::{
        ActiveModelTrait, ConnectionTrait, DatabaseBackend, EntityTrait, QueryOrder, Set, Statement,
    },
};
use serde::{Deserialize, Serialize};

const DEFAULT_ADDR: &str = "127.0.0.1:3000";
const DEFAULT_DATABASE_DIR: &str = "target/todos_api";
const DEFAULT_DATABASE_URL: &str = "sqlite://target/todos_api/todos.sqlite?mode=rwc";
const CREATE_TODOS_TABLE: &str = "CREATE TABLE IF NOT EXISTS todos (id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, title TEXT NOT NULL, completed BOOLEAN NOT NULL)";

type ExampleResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Clone)]
struct TodosController;

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

#[derive(Clone, Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

impl From<entity::Model> for TodoResponse {
    fn from(todo: entity::Model) -> Self {
        Self {
            id: todo.id,
            title: todo.title,
            completed: todo.completed,
        }
    }
}

impl ResourceController<ApiState> for TodosController {
    type Id = i32;
    type Resource = TodoResponse;
    type Create = CreateTodo;
    type Update = UpdateTodo;

    fn index(&self, state: ApiState) -> ControllerFuture<'_, Vec<Self::Resource>> {
        Box::pin(async move {
            let database = state.database()?;
            let todos = entity::Entity::find()
                .order_by_asc(entity::Column::Id)
                .all(database)
                .await?;

            Ok(todos.into_iter().map(TodoResponse::from).collect())
        })
    }

    fn show(&self, state: ApiState, id: Self::Id) -> ControllerFuture<'_, Option<Self::Resource>> {
        Box::pin(async move {
            let database = state.database()?;
            let todo = entity::Entity::find_by_id(id).one(database).await?;

            Ok(todo.map(TodoResponse::from))
        })
    }

    fn create(&self, state: ApiState, input: Self::Create) -> ControllerFuture<'_, Self::Resource> {
        Box::pin(async move {
            let database = state.database()?;
            let inserted = entity::ActiveModel {
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
            let Some(todo) = entity::Entity::find_by_id(id).one(database).await? else {
                return Ok(None);
            };

            let mut active: entity::ActiveModel = todo.into();
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
            let result = entity::Entity::delete_by_id(id).exec(database).await?;

            Ok(result.rows_affected == 1)
        })
    }

    fn resource_id(&self, resource: &Self::Resource) -> Self::Id {
        resource.id
    }
}

#[tokio::main]
async fn main() -> ExampleResult<()> {
    init_tracing();

    let addr = env::var("RUW_TODOS_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_owned());
    let database_url = database_url()?;

    tracing::info!(%addr, %database_url, "starting todos_api example");

    let state = ApiState::connect_sqlite(&database_url).await?;
    create_schema(&state).await?;

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let bound_addr = listener.local_addr()?;

    let api = Api::new(state)
        .route("/health", get(health))
        .resource("/todos", TodosController);

    announce_startup(bound_addr);
    api.serve(listener).await?;

    Ok(())
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .try_init();
}

fn database_url() -> ExampleResult<String> {
    match env::var("RUW_TODOS_DATABASE_URL") {
        Ok(database_url) => Ok(database_url),
        Err(env::VarError::NotPresent) => {
            fs::create_dir_all(Path::new(DEFAULT_DATABASE_DIR))?;
            Ok(DEFAULT_DATABASE_URL.to_owned())
        }
        Err(error) => Err(Box::new(error)),
    }
}

async fn create_schema(state: &ApiState) -> ApiResult<()> {
    state
        .database()?
        .execute(Statement::from_string(
            DatabaseBackend::Sqlite,
            CREATE_TODOS_TABLE.to_owned(),
        ))
        .await?;

    tracing::info!(table = "todos", "SQLite schema is ready");
    Ok(())
}

async fn health(State(state): State<ApiState>) -> ApiResult<Json<HealthResponse>> {
    state
        .database()?
        .query_one(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT 1".to_owned(),
        ))
        .await?;

    Ok(Json(HealthResponse { status: "ok" }))
}

fn announce_startup(bound_addr: SocketAddr) {
    tracing::info!(%bound_addr, "todos_api example is listening");
    println!("todos_api listening on http://{bound_addr}");
}

fn validate_title(title: &str) -> ApiResult<()> {
    if title.trim().is_empty() {
        Err(ApiError::ValidationFailed)
    } else {
        Ok(())
    }
}
