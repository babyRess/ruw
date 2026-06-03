use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use ruw::cli::{
    parse_args, run, CliCommand, FieldKind, FieldSpec, GenerateCommand, NewCommand,
    ResourceCommand, HELP_TEXT,
};

#[test]
fn cli_parse_new_command_requires_path_argument() {
    let err = parse_args(["ruw", "new"]).expect_err("new requires a path");

    assert_eq!(err.phase(), "new");
    assert_eq!(err.invalid_argument(), Some("path"));
    assert!(err.message().contains("project path"));
}

#[test]
fn cli_parse_new_command_parses_path() {
    let command = parse_args(["ruw", "new", "./my-app"]).expect("new command should parse");

    assert_eq!(
        command,
        CliCommand::New(NewCommand {
            path: PathBuf::from("./my-app"),
        })
    );
}

#[test]
fn cli_parse_new_command_rejects_extra_arguments() {
    let error =
        parse_args(["ruw", "new", "./my-app", "unexpected"]).expect_err("new rejects extras");

    assert_eq!(error.phase(), "new");
    assert_eq!(error.invalid_argument(), Some("unexpected"));
}

#[test]
fn cli_parse_generate_resource_parses_name_and_fields() {
    let command = parse_args([
        "ruw",
        "generate",
        "resource",
        "Todo",
        "title:string",
        "done:boolean",
    ])
    .expect("generate resource should parse");

    assert_eq!(
        command,
        CliCommand::Generate(GenerateCommand::Resource(ResourceCommand {
            name: "Todo".to_owned(),
            output_path: PathBuf::from("."),
            fields: vec![
                FieldSpec {
                    name: "title".to_owned(),
                    kind: FieldKind::String,
                },
                FieldSpec {
                    name: "done".to_owned(),
                    kind: FieldKind::Boolean,
                },
            ],
        }))
    );
}

#[test]
fn cli_parse_generate_resource_supports_output_path_flag() {
    let command = parse_args([
        "ruw",
        "generate",
        "resource",
        "Todo",
        "--path",
        "./generated",
        "title:string",
    ])
    .expect("generate resource should parse with output path");

    assert_eq!(
        command,
        CliCommand::Generate(GenerateCommand::Resource(ResourceCommand {
            name: "Todo".to_owned(),
            output_path: PathBuf::from("./generated"),
            fields: vec![FieldSpec {
                name: "title".to_owned(),
                kind: FieldKind::String,
            }],
        }))
    );
}

#[test]
fn cli_parse_generate_resource_without_fields_is_rejected() {
    let error = parse_args(["ruw", "generate", "resource", "Todo"])
        .expect_err("generate resource requires at least one field");

    assert_eq!(error.phase(), "generate.resource");
    assert_eq!(error.invalid_argument(), Some("field-spec"));
}

#[test]
fn cli_parse_generate_resource_rejects_malformed_field_spec() {
    let error = parse_args(["ruw", "generate", "resource", "Todo", "title"])
        .expect_err("field-spec without colon should fail");

    assert_eq!(error.phase(), "field-spec");
    assert_eq!(error.invalid_argument(), Some("title"));
    assert!(error.message().contains("<name>:<type>"));
}

#[test]
fn cli_parse_generate_resource_rejects_unknown_field_type() {
    let error = parse_args(["ruw", "generate", "resource", "Todo", "title:weird"])
        .expect_err("unknown field type should fail");

    assert_eq!(error.phase(), "field-spec");
    assert_eq!(error.invalid_argument(), Some("title:weird"));
    assert!(error.message().contains("unsupported field type"));
}

#[test]
fn cli_parse_generate_resource_rejects_reserved_generated_id_field() {
    let error = parse_args(["ruw", "generate", "resource", "Todo", "id:integer"])
        .expect_err("generated resources own the id field");

    assert_eq!(error.phase(), "field-spec");
    assert_eq!(error.invalid_argument(), Some("id:integer"));
    assert!(error.message().contains("reserved"));
}

#[test]
fn cli_parse_generate_resource_rejects_rust_keywords() {
    let error = parse_args(["ruw", "generate", "resource", "type", "title:string"])
        .expect_err("resource name should reject Rust keywords");

    assert_eq!(error.phase(), "generate.resource");
    assert_eq!(error.invalid_argument(), Some("type"));
}

#[test]
fn cli_parse_unknown_command_returns_command_phase_error() {
    let error = parse_args(["ruw", "rebuild"]).expect_err("unknown command should fail");

    assert_eq!(error.phase(), "command");
    assert_eq!(error.invalid_argument(), Some("rebuild"));
    assert!(error.message().contains("unknown command"));
}

#[test]
fn cli_parse_help_flags_return_help_command() {
    assert_eq!(
        parse_args(["ruw", "--help"]).expect("help flag should parse"),
        CliCommand::Help
    );
    assert_eq!(
        parse_args(["ruw", "help"]).expect("help subcommand should parse"),
        CliCommand::Help
    );
}

#[test]
fn cli_help_text_lists_core_commands() {
    assert!(HELP_TEXT.contains("ruw new <path>"));
    assert!(HELP_TEXT.contains("ruw generate resource <ResourceName> <field>:<type>..."));
}

#[test]
fn cli_error_contract_exposes_argument_in_messages() {
    let error = parse_args(["ruw", "generate", "resource", "Todo", "title"])
        .expect_err("field parse failure should be surfaced");

    let message = format!("{error}");
    assert!(message.contains("error [phase=field-spec]"));
    assert!(message.contains("invalid argument 'title'"));
    assert!(!message.to_lowercase().contains("backtrace"));
}

#[test]
fn cli_allow_program_name_omitted_for_direct_calls() {
    let command = parse_args(["generate", "resource", "Todo", "title:string"])
        .expect("command parser should work without program name token");

    assert_eq!(
        command,
        CliCommand::Generate(GenerateCommand::Resource(ResourceCommand {
            name: "Todo".to_owned(),
            output_path: PathBuf::from("."),
            fields: vec![FieldSpec {
                name: "title".to_owned(),
                kind: FieldKind::String,
            }],
        }))
    );
}

#[test]
fn cli_run_new_command_writes_project_scaffold() {
    let root_dir = temp_path("new_project_root");
    let _cleanup = Cleanup::new(&root_dir);
    let project_dir = root_dir.join("new_project");

    run(vec![
        "ruw".to_owned(),
        "new".to_owned(),
        project_dir.display().to_string(),
    ])
    .expect("new command should write a project scaffold");

    let cargo_toml = fs::read_to_string(project_dir.join("Cargo.toml"))
        .expect("generated Cargo.toml should be readable");
    let main_rs =
        fs::read_to_string(project_dir.join("src/main.rs")).expect("main.rs should be readable");

    assert!(cargo_toml.contains("name = \"new_project\""));
    assert!(cargo_toml.contains("ruw = { path = "));
    assert!(main_rs.contains(".route(\"/health\", get(health))"));
    assert!(main_rs.contains("ruw app listening on http://"));
}

#[test]
fn cli_run_new_command_rejects_non_empty_directory() {
    let project_dir = temp_path("non_empty_project");
    let _cleanup = Cleanup::new(&project_dir);
    fs::create_dir_all(&project_dir).expect("temp project directory should be created");
    fs::write(project_dir.join("existing.txt"), "keep me").expect("fixture should be written");

    let error = run(vec![
        "ruw".to_owned(),
        "new".to_owned(),
        project_dir.display().to_string(),
    ])
    .expect_err("new should not write into a non-empty directory");

    assert_eq!(error.phase(), "new");
    assert!(error.message().contains("not empty"));
    assert!(project_dir.join("existing.txt").exists());
}

#[test]
fn cli_run_generate_resource_writes_controller_scaffold() {
    let project_dir = temp_path("resource_project");
    let _cleanup = Cleanup::new(&project_dir);

    run(vec![
        "ruw".to_owned(),
        "new".to_owned(),
        project_dir.display().to_string(),
    ])
    .expect("new command should write a project scaffold");

    run(vec![
        "ruw".to_owned(),
        "generate".to_owned(),
        "resource".to_owned(),
        "Todo".to_owned(),
        "--path".to_owned(),
        project_dir.display().to_string(),
        "title:string".to_owned(),
        "completed:boolean".to_owned(),
    ])
    .expect("generate resource should write a resource module");

    let source =
        fs::read_to_string(project_dir.join("src/todo.rs")).expect("resource module should exist");
    assert!(source.contains("pub struct TodoController;"));
    assert!(source.contains("pub completed: bool,"));
    assert!(source.contains("pub completed: Option<bool>,"));
    assert!(source.contains("Api::new(state).resource(\"/todos\", TodoController)"));
}

#[test]
fn cli_run_generate_resource_refuses_to_overwrite_existing_file() {
    let project_dir = temp_path("existing_resource_project");
    let _cleanup = Cleanup::new(&project_dir);
    fs::create_dir_all(project_dir.join("src")).expect("source directory should be created");
    fs::write(project_dir.join("src/todo.rs"), "existing").expect("fixture should be written");

    let error = run(vec![
        "ruw".to_owned(),
        "generate".to_owned(),
        "resource".to_owned(),
        "Todo".to_owned(),
        "--path".to_owned(),
        project_dir.display().to_string(),
        "title:string".to_owned(),
    ])
    .expect_err("generate resource should not overwrite an existing module");

    assert_eq!(error.phase(), "generate.resource");
    assert!(error.message().contains("overwrite"));
    assert_eq!(
        fs::read_to_string(project_dir.join("src/todo.rs")).expect("fixture should remain"),
        "existing"
    );
}

struct Cleanup {
    path: PathBuf,
}

impl Cleanup {
    fn new(path: &Path) -> Self {
        let _ = fs::remove_dir_all(path);
        Self {
            path: path.to_owned(),
        }
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn temp_path(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after Unix epoch")
        .as_nanos();

    std::env::temp_dir().join(format!("ruw_cli_{label}_{}_{}", std::process::id(), unique))
}
