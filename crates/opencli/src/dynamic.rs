//! `opencli <api> <resource…> <verb>`: commands built at runtime from a
//! [`CommandSet`], with no code generation (ADR 0001).

use std::collections::BTreeMap;
use std::io::Write as _;
use std::time::{Duration, Instant};

use clap::builder::{PossibleValuesParser, ValueParser};
use clap::{Arg, ArgAction, ArgMatches, Command};
use opencli_auth::{TokenClient, place, select};
use opencli_core::ApiName;
use opencli_core::{
    ArgumentSpec, ArgumentTarget, BodySpec, CommandSpec, HttpMethod, ValueType, is_form_media_type,
    is_json_media_type,
};
use opencli_runtime::{
    CredentialPlacement, HttpClient, RequestPlan, Response, is_secure_transport, plan_request,
    typed_value,
};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::io::AsyncReadExt;
use url::Url;

use crate::audit::{self, AuditEvent};
use crate::cache::CachedCommands;
use crate::cli::OutputFormat;
use crate::credentials;
use crate::error::CliError;
use crate::output::{self, emit};
use crate::paths::Paths;
use crate::{progress, prompt, server};

const DESCRIBE: &str = "describe";
const DRY_RUN: &str = "dry-run";
const BODY: &str = "body";
const YES: &str = "yes";
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

/// Global options that apply to every operation command.
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub format: OutputFormat,
    pub server: Option<String>,
    pub timeout: Duration,
}

/// Runs the leaf command selected in `matches` (the API's own matches).
pub async fn run(
    matches: &ArgMatches,
    cached: &CachedCommands,
    options: &RunOptions,
    paths: &Paths,
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
    let dry_run = current.get_flag(DRY_RUN);
    let server = match server::resolve(
        options.server.as_deref(),
        &cached.commands.servers,
        &cached.source,
    ) {
        Ok(server) => Some(server),
        // A dry run can still show the request relative to the spec's own server.
        Err(_) if dry_run => cached.commands.servers.first().cloned(),
        Err(error) => return Err(error),
    };
    let mut plan = plan_request(command, server.as_deref(), &values, body)?;
    tracing::debug!(method = %plan.method, url = %plan.url, "request planned");
    // Dry runs never touch credentials: no passphrase prompts, no token refreshes.
    if dry_run {
        return emit(options.format, &plan, render_plan);
    }
    // `--yes` only exists on commands that change data.
    let confirmed = command.method == HttpMethod::Get || current.get_flag(YES);
    if !confirmed && !confirm_change(&plan)? {
        return Ok(());
    }

    let api = ApiName::parse(&cached.api).map_err(|error| CliError::Internal(error.to_string()))?;
    let started = Instant::now();
    let (credentials, schemes) =
        authorize(&api, command, cached, &plan, options.timeout, paths).await?;
    tracing::debug!(elapsed_ms = started.elapsed().as_millis(), schemes = ?schemes, "credentials resolved");
    plan.credentials = credentials;
    let sent_credentials = !plan.credentials.is_empty();

    let started = Instant::now();
    let label = format!("{} {}", plan.method, plan.url);
    let sent = progress::with_spinner(label, async {
        HttpClient::new(options.timeout)?.send(&plan).await
    })
    .await;
    let elapsed = started.elapsed();
    let audit_event = |status: Option<u16>, outcome: &'static str| AuditEvent::Request {
        api: api.as_str(),
        command: command.path.join(" "),
        method: plan.method.to_string(),
        url: &plan.url,
        status,
        outcome,
        duration_ms: elapsed.as_millis(),
        schemes: schemes.clone(),
    };
    let response = match sent {
        Ok(response) => response,
        Err(error) => {
            audit::record(paths, &audit_event(None, "network-error")).await;
            return Err(error.into());
        }
    };
    let outcome = if response.is_success() {
        "ok"
    } else {
        "http-error"
    };
    audit::record(paths, &audit_event(Some(response.status), outcome)).await;
    print_response(&response, options.format)?;
    if response.is_success() {
        return Ok(());
    }
    Err(CliError::HttpStatus {
        help: rejection_help(response.status, sent_credentials, &api),
        method: plan.method.to_string(),
        url: plan.url,
        status: response.status,
        reason: response.reason,
    })
}

/// Picks credentials for the operation (ADR 0004), refreshing and saving
/// tokens as needed, and refuses to send them over plain HTTP.
async fn authorize(
    api: &ApiName,
    command: &CommandSpec,
    cached: &CachedCommands,
    plan: &RequestPlan,
    timeout: Duration,
    paths: &Paths,
) -> Result<(Vec<CredentialPlacement>, Vec<String>), CliError> {
    if command.security.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let store = credentials::open(paths)?;
    let selection = {
        let (store, api) = (Arc::clone(&store), api.clone());
        let requirements = command.security.clone();
        let schemes = cached.commands.security_schemes.clone();
        credentials::blocking(move || {
            select(&api, &requirements, &schemes, store.as_ref(), &|name| {
                std::env::var(name).ok()
            })
        })
        .await?
    };
    let tokens = TokenClient::new(timeout)?;
    let schemes: Vec<String> = selection
        .items
        .iter()
        .map(|item| item.scheme_name.clone())
        .collect();
    let (placements, refreshed) = place(api, selection, &tokens, OffsetDateTime::now_utc()).await?;
    if !placements.is_empty() && !Url::parse(&plan.url).is_ok_and(|url| is_secure_transport(&url)) {
        return Err(CliError::InsecureCredentials(plan.url.clone()));
    }
    if !refreshed.is_empty() {
        let saved = credentials::blocking(move || {
            for token in refreshed {
                store.set(&token.key, &token.credential)?;
            }
            Ok(())
        })
        .await;
        // The request can still go ahead; the token is just refreshed again next time.
        if let Err(error) = saved {
            tracing::warn!(%api, %error, "could not save refreshed credentials");
        }
    }
    Ok((placements, schemes))
}

fn rejection_help(status: u16, sent_credentials: bool, api: &ApiName) -> Option<String> {
    match (status, sent_credentials) {
        (401 | 403, true) => Some(format!(
            "The server rejected the credentials. Replace them with `opencli auth login {api}`."
        )),
        (401, false) => Some(format!(
            "This operation may need credentials: `opencli auth login {api}`."
        )),
        _ => None,
    }
}

/// Requests that change data need consent: a prompt in a terminal, `--yes` elsewhere.
fn confirm_change(plan: &RequestPlan) -> Result<bool, CliError> {
    if !prompt::is_interactive() {
        return Err(CliError::SendConfirmationRequired {
            method: plan.method.to_string(),
            url: plan.url.clone(),
        });
    }
    prompt::confirm(&format!("Send {} {}?", plan.method, plan.url))
}

/// JSON goes through `--output`; text and binary bodies are written as they came.
fn print_response(response: &Response, format: OutputFormat) -> Result<(), CliError> {
    if let Some(json) = response.json() {
        return emit(format, &json, output::json_table);
    }
    let mut stdout = std::io::stdout().lock();
    let written = if response.body.is_empty() {
        if format == OutputFormat::Table && response.is_success() {
            writeln!(stdout, "HTTP {} {}", response.status, response.reason)
        } else {
            Ok(())
        }
    } else {
        stdout.write_all(&response.body).and_then(|()| {
            if response.text().is_some_and(|text| !text.ends_with('\n')) {
                writeln!(stdout)
            } else {
                Ok(())
            }
        })
    };
    written.map_err(CliError::Terminal)
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
    if spec.method != HttpMethod::Get {
        command = command.arg(
            Arg::new(YES)
                .long(YES)
                .action(ArgAction::SetTrue)
                .help("Send without asking for confirmation (required when not in a terminal)"),
        );
    }
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
    if is_json_media_type(&spec.content_type) {
        return serde_json::from_str(&text).map_err(CliError::BodyJson);
    }
    // Form bodies take a JSON object of fields or an already-encoded string.
    if is_form_media_type(&spec.content_type)
        && let Ok(fields @ Value::Object(_)) = serde_json::from_str::<Value>(&text)
    {
        return Ok(fields);
    }
    Ok(Value::String(text))
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
