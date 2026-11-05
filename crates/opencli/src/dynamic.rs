//! `opencli <api> <resource…> <verb>`: commands built at runtime from a
//! [`CommandSet`], with no code generation (ADR 0001).

use std::collections::BTreeMap;

use clap::builder::{PossibleValuesParser, ValueParser};
use clap::{Arg, ArgAction, ArgMatches, Command};
use opencli_core::{ArgumentSpec, ArgumentTarget, BodySpec, CommandSpec, HttpMethod, ValueType};
use opencli_runtime::{RequestPlan, plan_request, typed_value};
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncReadExt;

use crate::cache::CachedCommands;
use crate::cli::OutputFormat;
use crate::error::CliError;
use crate::output::emit;

const DESCRIBE: &str = "describe";
const DRY_RUN: &str = "dry-run";
const BODY: &str = "body";
const STDIN_MARKER: &str = "-";
const FILE_PREFIX: char = '@';

/// The clap subtree for one API.
pub fn api_command(cached: &CachedCommands) -> Command {
    let mut tree = Node::default();
    for (index, command) in cached.commands.commands.iter().enumerate() {
        tree.insert(&command.path, index);
    }
    let root =
        Command::new(cached.api.clone()).about(format!("{} {}", cached.title, cached.version));
    tree.build(root, &cached.commands.commands)
}

/// Runs the leaf command selected in `matches` (the API's own matches).
pub async fn run(
    matches: &ArgMatches,
    cached: &CachedCommands,
    format: OutputFormat,
) -> Result<(), CliError> {
    let mut words = Vec::new();
    let mut current = matches;
    while let Some((name, sub)) = current.subcommand() {
        words.push(name.to_owned());
        current = sub;
    }
    let command = cached
        .commands
        .commands
        .iter()
        .find(|command| command.path == words)
        .ok_or_else(|| CliError::UnknownCommand(words.join(" ")))?;

    if current.get_flag(DESCRIBE) {
        let contract = OperationContract::new(cached, command);
        // `--describe` is a machine contract, so it is JSON regardless of `--output`.
        return emit(OutputFormat::Json, &contract, |_| String::new());
    }

    let values: Vec<Option<Vec<String>>> = (0..command.arguments.len())
        .map(|index| {
            current
                .get_many::<String>(argument_id(index).as_str())
                .map(|values| values.cloned().collect())
        })
        .collect();
    // `--body` only exists on commands whose operation takes a body.
    let body = match &command.body {
        Some(spec) => match current.get_one::<String>(BODY) {
            Some(raw) => Some(read_body(raw, spec).await?),
            None => None,
        },
        None => None,
    };
    let plan = plan_request(command, &cached.commands.servers, &values, body)?;
    if current.get_flag(DRY_RUN) {
        return emit(format, &plan, render_plan);
    }
    Err(CliError::SendingNotImplemented)
}

#[derive(Default)]
struct Node {
    children: BTreeMap<String, Node>,
    leaf: Option<usize>,
}

impl Node {
    fn insert(&mut self, path: &[String], index: usize) {
        match path.split_first() {
            None => self.leaf = Some(index),
            Some((head, rest)) => self
                .children
                .entry(head.clone())
                .or_default()
                .insert(rest, index),
        }
    }

    /// Naming guarantees a leaf never has children (`NamingError` otherwise).
    fn build(&self, command: Command, specs: &[CommandSpec]) -> Command {
        if let Some(index) = self.leaf {
            return leaf_command(command, &specs[index]);
        }
        let verbs: Vec<&str> = self.children.keys().map(String::as_str).collect();
        let group = command
            .about(verbs.join(", "))
            .subcommand_required(true)
            .arg_required_else_help(true)
            // A resource may legitimately be called `help`.
            .disable_help_subcommand(true);
        self.children.iter().fold(group, |group, (name, child)| {
            group.subcommand(child.build(Command::new(name.clone()), specs))
        })
    }
}

fn leaf_command(command: Command, spec: &CommandSpec) -> Command {
    let summary = spec.summary.clone().unwrap_or_default();
    let about = if spec.deprecated {
        format!("[deprecated] {summary}").trim_end().to_owned()
    } else {
        summary
    };
    let mut command = command
        .about(about)
        .after_help(format!("Calls {} {}", spec.method, spec.path_template));
    if let Some(description) = &spec.description {
        command = command.long_about(description.clone());
    }
    command = spec
        .arguments
        .iter()
        .enumerate()
        .fold(command, |command, (index, argument)| {
            command.arg(clap_argument(index, argument))
        });
    command = command
        .arg(
            Arg::new(DESCRIBE)
                .long(DESCRIBE)
                .exclusive(true)
                .action(ArgAction::SetTrue)
                .help("Print this operation's arguments and body schema as JSON, then exit"),
        )
        .arg(
            Arg::new(DRY_RUN)
                .long(DRY_RUN)
                .action(ArgAction::SetTrue)
                .help("Print the HTTP request instead of sending it"),
        );
    if let Some(body) = &spec.body {
        command = command.arg(
            Arg::new(BODY)
                .long(BODY)
                .value_name("JSON|@FILE|-")
                .help(format!(
                    "Request body ({}): inline, @file, or - for stdin",
                    body.content_type
                )),
        );
    }
    command
}

fn argument_id(index: usize) -> String {
    format!("argument-{index}")
}

fn clap_argument(index: usize, argument: &ArgumentSpec) -> Arg {
    let help = argument
        .help
        .clone()
        .unwrap_or_else(|| default_help(&argument.target));
    let mut arg = Arg::new(argument_id(index))
        .value_parser(value_parser(argument))
        .help(help);
    // `--describe` is exclusive, which lifts `required` while it is present.
    arg = arg.required(argument.required);
    if argument.positional {
        return arg.value_name(argument.name.to_ascii_uppercase().replace('-', "_"));
    }
    arg.long(argument.name.clone())
        .value_name(type_name(argument.value_type).to_ascii_uppercase())
        .action(if argument.multiple {
            ArgAction::Append
        } else {
            ArgAction::Set
        })
}

fn default_help(target: &ArgumentTarget) -> String {
    match target {
        ArgumentTarget::Path(name) => format!("Path parameter `{name}`"),
        ArgumentTarget::Query(name) => format!("Query parameter `{name}`"),
        ArgumentTarget::Header(name) => format!("Header `{name}`"),
        ArgumentTarget::Cookie(name) => format!("Cookie `{name}`"),
        ArgumentTarget::BodyField(name) => format!("Request body field `{name}`"),
    }
}

fn value_parser(argument: &ArgumentSpec) -> ValueParser {
    if !argument.allowed_values.is_empty() {
        return PossibleValuesParser::new(argument.allowed_values.clone()).into();
    }
    let value_type = argument.value_type;
    ValueParser::new(move |raw: &str| -> Result<String, String> {
        typed_value("", raw, value_type)
            .map(|_| raw.to_owned())
            .map_err(|_| format!("expected {}", type_name(value_type)))
    })
}

const fn type_name(value_type: ValueType) -> &'static str {
    match value_type {
        ValueType::String => "string",
        ValueType::Integer => "integer",
        ValueType::Number => "number",
        ValueType::Boolean => "boolean",
    }
}

/// Reads `--body`: inline text, `@file`, or `-` for stdin. JSON bodies are parsed.
async fn read_body(raw: &str, spec: &BodySpec) -> Result<Value, CliError> {
    let text = if raw == STDIN_MARKER {
        let mut text = String::new();
        tokio::io::stdin()
            .read_to_string(&mut text)
            .await
            .map_err(CliError::Terminal)?;
        text
    } else if let Some(path) = raw.strip_prefix(FILE_PREFIX) {
        tokio::fs::read_to_string(path)
            .await
            .map_err(|source| CliError::BodyFile {
                path: path.to_owned(),
                source,
            })?
    } else {
        raw.to_owned()
    };
    if is_json(&spec.content_type) {
        serde_json::from_str(&text).map_err(CliError::BodyJson)
    } else {
        Ok(Value::String(text))
    }
}

fn is_json(content_type: &str) -> bool {
    let essence = content_type.split(';').next().unwrap_or_default().trim();
    essence == "application/json" || essence.ends_with("+json")
}

fn render_plan(plan: &RequestPlan) -> String {
    let mut url = plan.url.clone();
    if !plan.query.is_empty() {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(&plan.query)
            .finish();
        url = format!("{url}?{query}");
    }
    let mut lines = vec![format!("{} {url}", plan.method)];
    lines.extend(
        plan.headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}")),
    );
    if !plan.cookies.is_empty() {
        let cookies: Vec<String> = plan
            .cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        lines.push(format!("Cookie: {}", cookies.join("; ")));
    }
    if let Some(body) = &plan.body {
        lines.push(format!("Content-Type: {}", body.content_type));
        lines.push(String::new());
        lines.push(match &body.value {
            Value::String(text) => text.clone(),
            value => serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
        });
    }
    lines.join("\n")
}

/// `--describe` output: everything an agent needs to call the operation.
#[derive(Serialize)]
struct OperationContract<'a> {
    command: String,
    method: HttpMethod,
    path: &'a str,
    operation_id: Option<&'a str>,
    summary: Option<&'a str>,
    description: Option<&'a str>,
    deprecated: bool,
    servers: &'a [String],
    arguments: Vec<DescribedArgument<'a>>,
    body: Option<&'a BodySpec>,
}

#[derive(Serialize)]
struct DescribedArgument<'a> {
    /// How to pass it: `--flag` or `<POSITIONAL>`.
    usage: String,
    #[serde(flatten)]
    spec: &'a ArgumentSpec,
}

impl<'a> OperationContract<'a> {
    fn new(cached: &'a CachedCommands, command: &'a CommandSpec) -> Self {
        let arguments = command
            .arguments
            .iter()
            .map(|spec| DescribedArgument {
                usage: if spec.positional {
                    format!("<{}>", spec.name.to_ascii_uppercase().replace('-', "_"))
                } else {
                    format!("--{}", spec.name)
                },
                spec,
            })
            .collect();
        Self {
            command: format!("opencli {} {}", cached.api, command.path.join(" ")),
            method: command.method,
            path: &command.path_template,
            operation_id: command.operation_id.as_deref(),
            summary: command.summary.as_deref(),
            description: command.description.as_deref(),
            deprecated: command.deprecated,
            servers: &cached.commands.servers,
            arguments,
            body: command.body.as_ref(),
        }
    }
}
