//! Lightweight CLI parser for the `ruw` generator workflow.
//!
//! The parser intentionally stays small and boring: command parsing is split into
//! explicit small functions with stable, testable error reporting.

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

/// Top-level CLI command parsed from `ruw` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    /// Display CLI help text.
    Help,
    /// `ruw new <path>`.
    New(NewCommand),
    /// `ruw generate ...`.
    Generate(GenerateCommand),
}

/// Parsed form of `ruw new <path>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCommand {
    /// Project output path.
    pub path: PathBuf,
}

/// Parsed form of `ruw generate ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenerateCommand {
    /// `ruw generate resource <Name> <field>:<type>...`.
    Resource(ResourceCommand),
}

/// Parsed form of `ruw generate resource`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceCommand {
    /// Resource name, such as `Todo`.
    pub name: String,
    /// Field specs such as `title:string` and `done:boolean`.
    pub fields: Vec<FieldSpec>,
    /// Optional target output path. Defaults to current directory.
    pub output_path: PathBuf,
}

/// Parsed `name:type` field spec from generate input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSpec {
    /// Field name.
    pub name: String,
    /// Field type normalized to known CLI types.
    pub kind: FieldKind,
}

/// Supported generator-friendly field types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// UTF-8 text field.
    String,
    /// True/false field.
    Boolean,
    /// Signed/unsigned integer field.
    Integer,
    /// Floating-point field.
    Float,
    /// UUID field.
    Uuid,
}

/// Stable parser error contract used by CLI-facing code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    phase: &'static str,
    invalid_argument: Option<String>,
    message: String,
}

impl CliError {
    /// CLI parsing phase where the error was detected.
    pub fn phase(&self) -> &'static str {
        self.phase
    }

    /// The user-provided argument that triggered the failure, if known.
    pub fn invalid_argument(&self) -> Option<&str> {
        self.invalid_argument.as_deref()
    }

    /// Human readable summary for UI/terminal consumers.
    pub fn message(&self) -> &str {
        &self.message
    }

    fn with_invalid_argument(
        phase: &'static str,
        argument: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            phase,
            invalid_argument: Some(argument.into()),
            message: message.into(),
        }
    }

    fn missing_argument(
        phase: &'static str,
        expected: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            phase,
            invalid_argument: Some(expected.into()),
            message: message.into(),
        }
    }

    fn io_failure(
        phase: &'static str,
        path: &Path,
        action: &'static str,
        error: io::Error,
    ) -> Self {
        Self {
            phase,
            invalid_argument: Some(path.display().to_string()),
            message: format!("{action}: {error}"),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.invalid_argument.as_deref() {
            Some(argument) => write!(
                f,
                "error [phase={}] invalid argument '{}': {}",
                self.phase, argument, self.message
            ),
            None => write!(f, "error [phase={}]: {}", self.phase, self.message),
        }
    }
}

impl std::error::Error for CliError {}

impl FieldKind {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "string" | "str" => Some(Self::String),
            "bool" | "boolean" => Some(Self::Boolean),
            "int" | "integer" | "i32" | "i64" | "u32" | "u64" => Some(Self::Integer),
            "float" | "f32" | "f64" => Some(Self::Float),
            "uuid" => Some(Self::Uuid),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Boolean => "boolean",
            Self::Integer => "integer",
            Self::Float => "float",
            Self::Uuid => "uuid",
        }
    }

    fn rust_type(self) -> &'static str {
        match self {
            Self::String | Self::Uuid => "String",
            Self::Boolean => "bool",
            Self::Integer => "i64",
            Self::Float => "f64",
        }
    }
}

impl FieldSpec {
    fn parse(raw: &str) -> Result<Self, CliError> {
        let mut parts = raw.split(':');
        let name = parts.next().unwrap_or("");
        let kind = parts.next().unwrap_or("");

        if parts.next().is_some() || name.is_empty() || kind.is_empty() {
            return Err(CliError::with_invalid_argument(
                "field-spec",
                raw,
                "field spec must be <name>:<type>",
            ));
        }

        if !is_identifier(name) || is_reserved_identifier(name) {
            return Err(CliError::with_invalid_argument(
                "field-spec",
                raw,
                "field name must be an identifier",
            ));
        }

        if name == "id" {
            return Err(CliError::with_invalid_argument(
                "field-spec",
                raw,
                "field name 'id' is reserved for the generated resource id",
            ));
        }

        let normalized = kind.to_lowercase();
        let kind = FieldKind::parse(&normalized).ok_or_else(|| {
            CliError::with_invalid_argument(
                "field-spec",
                raw,
                format!("unsupported field type '{kind}'"),
            )
        })?;

        Ok(Self {
            name: name.to_owned(),
            kind,
        })
    }
}

/// Public help banner for top-level and unknown command flows.
pub const HELP_TEXT: &str = r#"ruw generator CLI

USAGE:
  ruw <command>

COMMANDS:
  ruw new <path>
    create a project scaffold at <path>

  ruw generate resource <ResourceName> <field>:<type>...
    emit project scaffold pieces for a resource

FLAGS:
  -h, --help          Print this help text
  --path <path>        Set output path for generated files (defaults to '.')

EXAMPLES:
  ruw new ./my-app
  ruw generate resource Todo title:string completed:boolean
  ruw generate resource Note --path ./my-app title:string body:string
"#;

/// Parse arguments and return a command that can be handled by library logic.
pub fn parse_args<I, S>(args: I) -> Result<CliCommand, CliError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args
        .into_iter()
        .map(|value| value.as_ref().to_owned())
        .collect();

    let command_args = command_slice(&args);
    parse_command(command_args)
}

fn command_slice(args: &[String]) -> &[String] {
    if args.is_empty() {
        return &[];
    }

    let first = args[0].as_str();
    if first == "new" || first == "generate" || first == "help" || is_help_flag(first) {
        args
    } else {
        if args.len() == 1 {
            args
        } else {
            &args[1..]
        }
    }
}

fn parse_command(args: &[String]) -> Result<CliCommand, CliError> {
    if args.is_empty() {
        return Err(CliError::missing_argument(
            "command",
            "subcommand",
            "no command provided",
        ));
    }

    match args[0].as_str() {
        "help" | "-h" | "--help" => Ok(CliCommand::Help),
        "new" => parse_new(&args[1..]),
        "generate" => parse_generate(&args[1..]),
        command => Err(CliError::with_invalid_argument(
            "command",
            command,
            "unknown command",
        )),
    }
}

fn parse_new(args: &[String]) -> Result<CliCommand, CliError> {
    if args.is_empty() {
        return Err(CliError::missing_argument(
            "new",
            "path",
            "new command requires a project path",
        ));
    }

    if args.len() > 1 {
        return Err(CliError::with_invalid_argument(
            "new",
            &args[1],
            "new command accepts exactly one <path>",
        ));
    }

    if args[0].is_empty() {
        return Err(CliError::with_invalid_argument(
            "new",
            "path",
            "project path must not be empty",
        ));
    }

    Ok(CliCommand::New(NewCommand {
        path: PathBuf::from(&args[0]),
    }))
}

fn parse_generate(args: &[String]) -> Result<CliCommand, CliError> {
    if args.is_empty() {
        return Err(CliError::missing_argument(
            "generate",
            "generate-subcommand",
            "generate command requires a subcommand",
        ));
    }

    if args[0] == "resource" {
        parse_generate_resource(&args[1..])
    } else {
        Err(CliError::with_invalid_argument(
            "generate",
            &args[0],
            "unknown generate subcommand",
        ))
    }
}

fn parse_generate_resource(args: &[String]) -> Result<CliCommand, CliError> {
    if args.is_empty() {
        return Err(CliError::missing_argument(
            "generate.resource",
            "resource-name",
            "generate resource requires a resource name",
        ));
    }

    let name = &args[0];
    if !is_identifier(name) || is_reserved_identifier(name) {
        return Err(CliError::with_invalid_argument(
            "generate.resource",
            name,
            "resource name must be a valid identifier",
        ));
    }

    let mut fields = Vec::new();
    let mut output_path = PathBuf::from(".");
    let mut index = 1;

    while index < args.len() {
        if args[index] == "--path" {
            index += 1;
            if index >= args.len() {
                return Err(CliError::missing_argument(
                    "generate.resource",
                    "--path value",
                    "--path requires a directory path",
                ));
            }

            output_path = PathBuf::from(&args[index]);
            index += 1;
            continue;
        }

        if is_flag(&args[index]) {
            return Err(CliError::with_invalid_argument(
                "generate.resource",
                &args[index],
                "unknown generate.resource flag",
            ));
        }

        fields.push(FieldSpec::parse(&args[index])?);
        index += 1;
    }

    if fields.is_empty() {
        return Err(CliError::missing_argument(
            "generate.resource",
            "field-spec",
            "generate resource requires at least one field spec",
        ));
    }

    Ok(CliCommand::Generate(GenerateCommand::Resource(
        ResourceCommand {
            name: name.clone(),
            fields,
            output_path,
        },
    )))
}

fn is_help_flag(raw: &str) -> bool {
    matches!(raw, "-h" | "--help")
}

fn is_flag(raw: &str) -> bool {
    raw.starts_with('-')
}

fn is_identifier(raw: &str) -> bool {
    if raw.is_empty() {
        return false;
    }

    let mut chars = raw.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }

    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_reserved_identifier(raw: &str) -> bool {
    matches!(
        raw,
        "_" | "Self"
            | "abstract"
            | "as"
            | "async"
            | "await"
            | "become"
            | "box"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "do"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "final"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "macro"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "override"
            | "priv"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "try"
            | "type"
            | "typeof"
            | "union"
            | "unsafe"
            | "unsized"
            | "use"
            | "virtual"
            | "where"
            | "while"
            | "yield"
    )
}

fn create_project(command: &NewCommand) -> Result<Vec<PathBuf>, CliError> {
    let project_dir = &command.path;

    if project_dir.exists() {
        if !project_dir.is_dir() {
            return Err(CliError::with_invalid_argument(
                "new",
                project_dir.display().to_string(),
                "project path already exists and is not a directory",
            ));
        }

        let mut entries = fs::read_dir(project_dir).map_err(|error| {
            CliError::io_failure("new", project_dir, "read project directory", error)
        })?;
        if entries.next().is_some() {
            return Err(CliError::with_invalid_argument(
                "new",
                project_dir.display().to_string(),
                "project path already exists and is not empty",
            ));
        }
    } else {
        fs::create_dir_all(project_dir).map_err(|error| {
            CliError::io_failure("new", project_dir, "create project directory", error)
        })?;
    }

    let src_dir = project_dir.join("src");
    fs::create_dir_all(&src_dir)
        .map_err(|error| CliError::io_failure("new", &src_dir, "create source directory", error))?;

    let package_name = package_name_from_path(project_dir);
    let cargo_toml = project_dir.join("Cargo.toml");
    let main_rs = src_dir.join("main.rs");

    write_new_file(&cargo_toml, &new_project_cargo_toml(&package_name), "new")?;
    write_new_file(&main_rs, NEW_PROJECT_MAIN, "new")?;

    Ok(vec![cargo_toml, main_rs])
}

fn generate_resource(command: &ResourceCommand) -> Result<Vec<PathBuf>, CliError> {
    let output_path = &command.output_path;
    if output_path.exists() && !output_path.is_dir() {
        return Err(CliError::with_invalid_argument(
            "generate.resource",
            output_path.display().to_string(),
            "output path already exists and is not a directory",
        ));
    }

    let src_dir = output_path.join("src");
    fs::create_dir_all(&src_dir).map_err(|error| {
        CliError::io_failure(
            "generate.resource",
            &src_dir,
            "create source directory",
            error,
        )
    })?;

    let module_name = to_snake_case(&command.name);
    let module_path = src_dir.join(format!("{module_name}.rs"));
    write_new_file(
        &module_path,
        &resource_module_source(command),
        "generate.resource",
    )?;

    Ok(vec![module_path])
}

fn write_new_file(path: &Path, contents: &str, phase: &'static str) -> Result<(), CliError> {
    if path.exists() {
        return Err(CliError::with_invalid_argument(
            phase,
            path.display().to_string(),
            "refusing to overwrite existing file",
        ));
    }

    fs::write(path, contents)
        .map_err(|error| CliError::io_failure(phase, path, "write generated file", error))
}

fn package_name_from_path(path: &Path) -> String {
    let raw = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("ruw-app");
    let name = sanitize_package_name(raw);

    if name.is_empty() {
        "ruw-app".to_owned()
    } else if name
        .chars()
        .next()
        .is_some_and(|value| value.is_ascii_digit())
    {
        format!("ruw-{name}")
    } else {
        name
    }
}

fn sanitize_package_name(raw: &str) -> String {
    let mut sanitized = String::new();
    let mut last_was_dash = false;

    for character in raw.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() || character == '_' {
            sanitized.push(character);
            last_was_dash = false;
        } else if !last_was_dash {
            sanitized.push('-');
            last_was_dash = true;
        }
    }

    sanitized.trim_matches('-').to_owned()
}

fn new_project_cargo_toml(package_name: &str) -> String {
    let ruw_path = toml_escape(env!("CARGO_MANIFEST_DIR"));

    format!(
        r#"[package]
name = "{package_name}"
version = "0.1.0"
edition = "2021"

[dependencies]
ruw = {{ path = "{ruw_path}" }}
serde = {{ version = "1", features = ["derive"] }}
tokio = {{ version = "1", features = ["macros", "rt-multi-thread", "net"] }}
tracing-subscriber = "0.3"
"#
    )
}

fn toml_escape(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('"', "\\\"")
}

const NEW_PROJECT_MAIN: &str = r#"use ruw::{
    axum::{routing::get, Json},
    prelude::*,
};
use serde::Serialize;
use std::net::SocketAddr;

#[derive(Clone, Default)]
struct AppState;

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .try_init();

    let addr: SocketAddr = std::env::var("RUW_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:3000".to_owned())
        .parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound_addr = listener.local_addr()?;

    println!("ruw app listening on http://{bound_addr}");

    Api::new(AppState)
        .route("/health", get(health))
        .serve(listener)
        .await?;

    Ok(())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}
"#;

fn resource_module_source(command: &ResourceCommand) -> String {
    let module_name = to_snake_case(&command.name);
    let type_name = to_pascal_case(&module_name);
    let plural_name = pluralize_snake(&module_name);
    let response_fields = render_response_fields(&command.fields);
    let create_fields = render_create_fields(&command.fields);
    let update_fields = render_update_fields(&command.fields);
    let create_assignments = render_create_assignments(&command.fields);
    let update_assignments = render_update_assignments(&command.fields);

    format!(
        r#"use ruw::prelude::*;
use serde::{{Deserialize, Serialize}};
use std::sync::{{Arc, Mutex}};

#[derive(Clone, Default)]
pub struct {type_name}State {{
    store: Arc<Mutex<{type_name}Store>>,
}}

#[derive(Debug, Default)]
struct {type_name}Store {{
    next_id: i64,
    items: Vec<{type_name}Response>,
}}

#[derive(Clone)]
pub struct {type_name}Controller;

#[derive(Clone, Debug, Serialize)]
pub struct {type_name}Response {{
    pub id: i64,
{response_fields}}}

#[derive(Clone, Debug, Deserialize)]
pub struct Create{type_name} {{
{create_fields}}}

#[derive(Clone, Debug, Deserialize)]
pub struct Update{type_name} {{
{update_fields}}}

impl ResourceController<{type_name}State> for {type_name}Controller {{
    type Id = i64;
    type Resource = {type_name}Response;
    type Create = Create{type_name};
    type Update = Update{type_name};

    fn index(&self, state: {type_name}State) -> ControllerFuture<'_, Vec<Self::Resource>> {{
        Box::pin(async move {{
            let store = state.store.lock().map_err(|_| ApiError::Internal)?;

            Ok(store.items.clone())
        }})
    }}

    fn show(
        &self,
        state: {type_name}State,
        id: Self::Id,
    ) -> ControllerFuture<'_, Option<Self::Resource>> {{
        Box::pin(async move {{
            let store = state.store.lock().map_err(|_| ApiError::Internal)?;

            Ok(store.items.iter().find(|item| item.id == id).cloned())
        }})
    }}

    fn create(
        &self,
        state: {type_name}State,
        input: Self::Create,
    ) -> ControllerFuture<'_, Self::Resource> {{
        Box::pin(async move {{
            let mut store = state.store.lock().map_err(|_| ApiError::Internal)?;
            store.next_id += 1;

            let item = {type_name}Response {{
                id: store.next_id,
{create_assignments}            }};
            store.items.push(item.clone());

            Ok(item)
        }})
    }}

    fn update(
        &self,
        state: {type_name}State,
        id: Self::Id,
        input: Self::Update,
    ) -> ControllerFuture<'_, Option<Self::Resource>> {{
        Box::pin(async move {{
            let mut store = state.store.lock().map_err(|_| ApiError::Internal)?;
            let Some(item) = store.items.iter_mut().find(|item| item.id == id) else {{
                return Ok(None);
            }};

{update_assignments}            Ok(Some(item.clone()))
        }})
    }}

    fn delete(&self, state: {type_name}State, id: Self::Id) -> ControllerFuture<'_, bool> {{
        Box::pin(async move {{
            let mut store = state.store.lock().map_err(|_| ApiError::Internal)?;
            let Some(position) = store.items.iter().position(|item| item.id == id) else {{
                return Ok(false);
            }};

            store.items.remove(position);
            Ok(true)
        }})
    }}

    fn resource_id(&self, resource: &Self::Resource) -> Self::Id {{
        resource.id
    }}
}}

pub fn api(state: {type_name}State) -> Api<{type_name}State> {{
    Api::new(state).resource("/{plural_name}", {type_name}Controller)
}}
"#
    )
}

fn render_response_fields(fields: &[FieldSpec]) -> String {
    fields
        .iter()
        .map(|field| format!("    pub {}: {},\n", field.name, field.kind.rust_type()))
        .collect()
}

fn render_create_fields(fields: &[FieldSpec]) -> String {
    fields
        .iter()
        .map(|field| format!("    pub {}: {},\n", field.name, field.kind.rust_type()))
        .collect()
}

fn render_update_fields(fields: &[FieldSpec]) -> String {
    fields
        .iter()
        .map(|field| {
            format!(
                "    pub {}: Option<{}>,\n",
                field.name,
                field.kind.rust_type()
            )
        })
        .collect()
}

fn render_create_assignments(fields: &[FieldSpec]) -> String {
    fields
        .iter()
        .map(|field| format!("                {0}: input.{0},\n", field.name))
        .collect()
}

fn render_update_assignments(fields: &[FieldSpec]) -> String {
    fields
        .iter()
        .map(|field| {
            format!(
                "            if let Some({0}) = input.{0} {{\n                item.{0} = {0};\n            }}\n",
                field.name
            )
        })
        .collect()
}

fn to_snake_case(raw: &str) -> String {
    let mut snake = String::new();
    let mut previous_was_separator = false;

    for (index, character) in raw.chars().enumerate() {
        if character == '_' || character == '-' {
            if !snake.is_empty() && !previous_was_separator {
                snake.push('_');
            }
            previous_was_separator = true;
            continue;
        }

        if character.is_ascii_uppercase() {
            if index > 0 && !previous_was_separator {
                snake.push('_');
            }
            snake.push(character.to_ascii_lowercase());
        } else {
            snake.push(character);
        }
        previous_was_separator = false;
    }

    snake.trim_matches('_').to_owned()
}

fn to_pascal_case(snake: &str) -> String {
    let mut pascal = String::new();

    for part in snake.split('_').filter(|part| !part.is_empty()) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            pascal.push(first.to_ascii_uppercase());
            pascal.push_str(chars.as_str());
        }
    }

    pascal
}

fn pluralize_snake(snake: &str) -> String {
    if let Some(stem) = snake.strip_suffix('y') {
        format!("{stem}ies")
    } else if snake.ends_with('s')
        || snake.ends_with('x')
        || snake.ends_with("ch")
        || snake.ends_with("sh")
    {
        format!("{snake}es")
    } else {
        format!("{snake}s")
    }
}

/// Execute the parsed command.
pub fn run<I, S>(args: I) -> Result<(), CliError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    match parse_args(args)? {
        CliCommand::Help => {
            println!("{HELP_TEXT}");
        }
        CliCommand::New(new) => {
            let files = create_project(&new)?;
            println!("created ruw project at {}", new.path.display());
            for file in files {
                println!("  {}", file.display());
            }
        }
        CliCommand::Generate(command) => match command {
            GenerateCommand::Resource(resource) => {
                let files = generate_resource(&resource)?;
                println!(
                    "generated resource {} in {}",
                    resource.name,
                    resource.output_path.display(),
                );
                for file in files {
                    println!("  {}", file.display());
                }
            }
        },
    }

    Ok(())
}

impl FieldKind {
    /// Render field kind as lower-case keyword for tests and templates.
    pub fn keyword(self) -> &'static str {
        self.as_str()
    }
}
