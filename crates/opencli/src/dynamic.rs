//! `opencli <api> <path words…> [method]`: commands built at runtime from a
//! [`CommandSet`], with no code generation (ADR 0001, ADR 0009).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write as _;
use std::time::{Duration, Instant};

use clap::builder::{PossibleValuesParser, ValueParser};
use clap::{Arg, ArgAction, ArgMatches, Command};
use opencli_auth::{
    AuthError, Credential, CredentialKey, TokenClient, environment_variable, place, select,
};
use opencli_core::{ApiName, SecurityScheme, SignIn, find_token, sign_in_target, token_scheme};
use opencli_core::{
    ArgumentSpec, ArgumentTarget, BodySpec, CommandSet, CommandSpec, HttpMethod, ValueType,
    is_form_media_type, is_json_media_type, is_multipart_media_type, is_secret_field,
    validate_body,
};
use opencli_plugin::{Format, Plugin, fetch_credential, render_columns};
use opencli_runtime::{
    CredentialPlacement, HttpClient, RequestPlan, Response, is_secure_transport, plan_request,
    typed_value,
};
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::io::AsyncReadExt;
use url::Url;

use crate::audit::{self, AuditEvent};
use crate::cache::CachedCommands;
use crate::cli::OutputFormat;
use crate::error::CliError;
use crate::fill::{self, FillMode};
use crate::output::{self, emit};
use crate::paths::Paths;
use crate::{cache, credentials};
use crate::{progress, prompt, server};

const DESCRIBE: &str = "describe";
const DRY_RUN: &str = "dry-run";
const BODY: &str = "body";
const YES: &str = "yes";
const INTERACTIVE: &str = "interactive";
const NO_VALIDATE: &str = "no-validate";
const NO_SAVE: &str = "no-save";
const MISSING_FIELD: &str = "required field is missing";
const STDIN_MARKER: &str = "-";
const FILE_PREFIX: char = '@';
/// Children named in a group's one-line description.
const LISTED_CHILDREN: usize = 6;

/// The clap subtree for one API.
pub fn api_command(cached: &CachedCommands) -> Command {
    let mut tree = Node::default();
    for (index, command) in cached.commands.commands.iter().enumerate() {
        tree.insert(&command.path, index);
    }
    let root =
        Command::new(cached.api.clone()).about(format!("{} {}", cached.title, cached.version));
    tree.build(root, &cached.api, &cached.commands)
}

/// The API's best sign-in command for `scheme`, if it has one (ADR 0010).
pub fn best_sign_in<'a>(
    cached: &'a CachedCommands,
    scheme: &str,
) -> Option<(&'a CommandSpec, SignIn)> {
    cached
        .commands
        .commands
        .iter()
        .filter_map(|command| {
            sign_in_target(command, &cached.commands.security_schemes)
                .filter(|target| target.scheme == scheme)
                .map(|target| (command, target))
        })
        .min_by_key(|(_, target)| target.rank)
}

/// Runs a sign-in command in the terminal (asking for its inputs) and saves
/// the token it returns; returns the name of the credential store.
pub async fn sign_in(
    cached: &CachedCommands,
    command: &CommandSpec,
    target: &SignIn,
    timeout: Duration,
    paths: &Paths,
) -> Result<&'static str, CliError> {
    // Every field, optional ones skippable: specs often leave a sign-in body
    // unmarked as required, and asking only for required values would send
    // nothing at all.
    let mut values = vec![None; command.arguments.len()];
    fill::arguments(
        command,
        &mut values,
        FillMode::Everything,
        &mut fill::Terminal,
    )?;
    let body = fill::body(
        command,
        None,
        &BTreeSet::new(),
        FillMode::Everything,
        &mut fill::Terminal,
    )?;
    let server = server::resolve(
        None,
        cached.base_url.as_deref(),
        &cached.commands.servers,
        &cached.source,
    )?;
    let plan = plan_request(command, Some(&server), &values, body)?;
    check_body(command, &plan)?;
    let api = ApiName::parse(&cached.api).map_err(|error| CliError::Internal(error.to_string()))?;
    let label = format!("{} {}", plan.method, plan.url);
    let started = Instant::now();
    let result =
        progress::with_spinner(label, async { HttpClient::new(timeout)?.send(&plan).await }).await;
    let response = result?;
    let event = AuditEvent::Request {
        api: api.as_str(),
        command: command.path.join(" "),
        method: plan.method.to_string(),
        url: &plan.url,
        status: Some(response.status),
        outcome: if response.is_success() {
            "ok"
        } else {
            "http-error"
        },
        duration_ms: started.elapsed().as_millis(),
        schemes: Vec::new(),
    };
    audit::record(paths, &event).await;
    if !response.is_success() {
        return Err(CliError::HttpStatus {
            help: Some("Check the values you entered and try again.".to_owned()),
            method: plan.method.to_string(),
            url: plan.url.clone(),
            status: response.status,
            reason: response.reason,
        });
    }
    let token = response
        .json()
        .as_ref()
        .and_then(find_token)
        .map(str::to_owned)
        .ok_or_else(|| CliError::NoTokenInResponse(operation_label(command)))?;
    let credential = credentials::from_token(target.form, &token);
    credentials::save(paths, &api, &target.scheme, credential).await
}

/// Global options that apply to every operation command.
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub query: Option<output::ResponseQuery>,
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
    extensions: &[Plugin],
) -> Result<(), CliError> {
    let (current, label, variants) = selected_command(matches, cached)?;

    if options.query.is_some() && (current.get_flag(DESCRIBE) || current.get_flag(DRY_RUN)) {
        return Err(CliError::ResponseQuery(
            "--query requires an HTTP response; omit --describe and --dry-run".to_owned(),
        ));
    }
    if current.get_flag(DESCRIBE) {
        let contract = CommandContract::new(cached, label, &variants);
        // `--describe` is a machine contract, so it is JSON regardless of `--output`.
        return emit(OutputFormat::Json, &contract, |_| String::new());
    }

    // `--body` only exists on commands with a variant that takes a body.
    let raw_body = variants
        .iter()
        .any(|variant| variant.body.is_some())
        .then(|| current.get_one::<String>(BODY))
        .flatten();
    // A body read from stdin leaves no terminal input for questions.
    let mode = fill_mode(current.get_flag(INTERACTIVE), raw_body.map(String::as_str))?;
    let command = choose_variant(&label, &variants, current, mode)?;
    check_flags_apply(command, &variants, current, raw_body.is_some())?;

    let mut values: Vec<Option<Vec<String>>> = command
        .arguments
        .iter()
        .map(|argument| {
            current
                .get_many::<String>(argument.name.as_str())
                .map(|values| values.cloned().collect())
        })
        .collect();
    let mut body = match (&command.body, raw_body) {
        (Some(spec), Some(raw)) => Some(read_body(raw, spec).await?),
        _ => None,
    };
    if let Some(mode) = mode {
        fill::arguments(command, &mut values, mode, &mut fill::Terminal)?;
        let flagged = flagged_body_fields(command, &values);
        body = fill::body(command, body, &flagged, mode, &mut fill::Terminal)?;
    } else {
        check_required(command, &values)?;
    }
    let dry_run = current.get_flag(DRY_RUN);
    let server = match server::resolve(
        options.server.as_deref(),
        cached.base_url.as_deref(),
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
    // `--no-validate` only exists on commands whose operation takes a body.
    if command.body.is_some() && !current.get_flag(NO_VALIDATE) {
        check_body(command, &plan)?;
    }
    // Dry runs never touch credentials: no passphrase prompts, no token refreshes.
    if dry_run {
        return emit(options.format, &plan, render_plan);
    }
    // `--yes` only exists on commands that change data.
    let confirmed = command.method == HttpMethod::Get || current.get_flag(YES);
    let whole_collection = variants
        .iter()
        .any(|variant| path_flags(variant).len() > path_flags(command).len());
    if !confirmed && !confirm_change(&plan, whole_collection)? {
        return Ok(());
    }

    let api = ApiName::parse(&cached.api).map_err(|error| CliError::Internal(error.to_string()))?;
    let started = Instant::now();
    let (credentials, schemes) = authorize(
        &api,
        command,
        cached,
        &plan,
        options.timeout,
        paths,
        extensions,
    )
    .await?;
    tracing::debug!(elapsed_ms = started.elapsed().as_millis(), schemes = ?schemes, "credentials resolved");
    plan.credentials = credentials;
    let columns = extensions
        .iter()
        .find_map(|plugin| plugin.formats.get(&command.path.join(" ")));
    // A sign-in's token becomes the API's credential unless `--no-save` (ADR 0010).
    let saves_token = !matches!(current.try_get_one::<bool>(NO_SAVE), Ok(Some(true)));
    let sent = Sent {
        api: &api,
        command,
        schemes,
        sign_in: sign_in_target(command, &cached.commands.security_schemes).filter(|_| saves_token),
    };
    send(&plan, &sent, options, paths, columns).await
}

/// The deepest subcommand matched, its full name, and its variants.
fn selected_command<'a>(
    matches: &'a ArgMatches,
    cached: &'a CachedCommands,
) -> Result<(&'a ArgMatches, String, Vec<&'a CommandSpec>), CliError> {
    let mut words = Vec::new();
    let mut current = matches;
    while let Some((name, sub)) = current.subcommand() {
        words.push(name.to_owned());
        current = sub;
    }
    let variants: Vec<&CommandSpec> = cached
        .commands
        .commands
        .iter()
        .filter(|command| command.path == words)
        .collect();
    if variants.is_empty() {
        return Err(CliError::UnknownCommand(words.join(" ")));
    }
    let label = format!("opencli {} {}", cached.api, words.join(" "))
        .trim_end()
        .to_owned();
    Ok((current, label, variants))
}

/// Questions need a person at a terminal: every value with `--interactive`,
/// otherwise just the required ones left out. Never when stdin is the body.
fn fill_mode(interactive: bool, raw_body: Option<&str>) -> Result<Option<FillMode>, CliError> {
    let terminal = prompt::is_interactive() && raw_body != Some(STDIN_MARKER);
    match (interactive, terminal) {
        (true, true) => Ok(Some(FillMode::Everything)),
        (true, false) => Err(CliError::InteractiveNeedsTerminal),
        (false, true) => Ok(Some(FillMode::MissingRequired)),
        (false, false) => Ok(None),
    }
}

/// Names of the flags that fill a variant's path parameters.
fn path_flags(command: &CommandSpec) -> BTreeSet<&str> {
    command
        .arguments
        .iter()
        .filter(|argument| matches!(argument.target, ArgumentTarget::Path(_)))
        .map(|argument| argument.name.as_str())
        .collect()
}

fn operation_label(command: &CommandSpec) -> String {
    format!("{} {}", command.method, command.path_template)
}

/// Picks the variant the path flags given point to (ADR 0009): an exact
/// match, else the only variant that takes them all. `-i` with no path flags,
/// or with an ambiguous set, asks which one.
fn choose_variant<'a>(
    label: &str,
    variants: &[&'a CommandSpec],
    matches: &ArgMatches,
    mode: Option<FillMode>,
) -> Result<&'a CommandSpec, CliError> {
    if let [only] = variants {
        return Ok(only);
    }
    let all_path_flags: BTreeSet<&str> = variants.iter().flat_map(|v| path_flags(v)).collect();
    let given: BTreeSet<&str> = all_path_flags
        .into_iter()
        .filter(|name| matches.get_raw(name).is_some())
        .collect();
    let asks = mode == Some(FillMode::Everything);
    if !(asks && given.is_empty()) {
        if let Some(exact) = variants.iter().find(|v| path_flags(v) == given) {
            return Ok(exact);
        }
        let candidates: Vec<&&CommandSpec> = variants
            .iter()
            .filter(|v| given.is_subset(&path_flags(v)))
            .collect();
        if let [only] = candidates.as_slice() {
            return Ok(only);
        }
    }
    if asks {
        let options: Vec<String> = variants.iter().map(|v| variant_line(v)).collect();
        let index = prompt::choose("Which one?", &options)?;
        return Ok(variants[index]);
    }
    let lines: Vec<String> = variants
        .iter()
        .map(|v| format!("  {}", variant_line(v)))
        .collect();
    Err(CliError::AmbiguousVariant {
        command: label.to_owned(),
        variants: lines.join("\n"),
    })
}

/// `GET /countries/{id}   --id <INTEGER>`
fn variant_line(command: &CommandSpec) -> String {
    let flags: Vec<String> = command
        .arguments
        .iter()
        .filter(|argument| matches!(argument.target, ArgumentTarget::Path(_)))
        .map(flag_usage)
        .collect();
    format!("{}   {}", operation_label(command), flags.join(" "))
        .trim_end()
        .to_owned()
}

fn flag_usage(argument: &ArgumentSpec) -> String {
    format!(
        "--{} <{}>",
        argument.name,
        type_name(argument.value_type).to_ascii_uppercase()
    )
}

/// Flags that belong to a sibling variant, not the chosen one, are mistakes.
fn check_flags_apply(
    command: &CommandSpec,
    variants: &[&CommandSpec],
    matches: &ArgMatches,
    body_given: bool,
) -> Result<(), CliError> {
    let own: BTreeSet<&str> = command
        .arguments
        .iter()
        .map(|argument| argument.name.as_str())
        .collect();
    let stray = variants
        .iter()
        .flat_map(|variant| &variant.arguments)
        .map(|argument| argument.name.as_str())
        .find(|name| !own.contains(name) && matches.get_raw(name).is_some())
        .or_else(|| (body_given && command.body.is_none()).then_some(BODY));
    match stray {
        Some(flag) => Err(CliError::FlagNotForVariant {
            flag: flag.to_owned(),
            operation: operation_label(command),
        }),
        None => Ok(()),
    }
}

/// Without a terminal nothing is asked, so required values must all be given.
/// clap checks this itself for commands with a single variant.
fn check_required(command: &CommandSpec, values: &[Option<Vec<String>>]) -> Result<(), CliError> {
    let missing = command
        .arguments
        .iter()
        .zip(values)
        .find(|(argument, value)| argument.required && value.is_none());
    match missing {
        Some((argument, _)) => Err(CliError::MissingArgument {
            flag: argument.name.clone(),
            operation: operation_label(command),
        }),
        None => Ok(()),
    }
}

/// Body fields already set by their own flags, so they aren't asked for again.
fn flagged_body_fields(command: &CommandSpec, values: &[Option<Vec<String>>]) -> BTreeSet<String> {
    command
        .arguments
        .iter()
        .zip(values)
        .filter_map(|(argument, given)| match (&argument.target, given) {
            (ArgumentTarget::BodyField(field), Some(_)) => Some(field.clone()),
            _ => None,
        })
        .collect()
}

/// Checks a JSON body against the operation's schema before anything is
/// sent (or shown by `--dry-run`).
pub(crate) fn check_body(command: &CommandSpec, plan: &RequestPlan) -> Result<(), CliError> {
    let (Some(spec), Some(body)) = (&command.body, &plan.body) else {
        return Ok(());
    };
    let Some(schema) = spec
        .schema
        .as_ref()
        .filter(|_| is_json_media_type(&spec.content_type))
    else {
        return Ok(());
    };
    let problems = validate_body(schema, &body.value);
    if problems.is_empty() {
        return Ok(());
    }
    let properties = schema.get("properties").and_then(Value::as_object);
    let missing_secrets: Vec<&str> = problems
        .iter()
        .filter(|problem| problem.message == MISSING_FIELD)
        .filter_map(|problem| problem.path.strip_prefix("body."))
        .filter(|name| {
            properties
                .and_then(|properties| properties.get(*name))
                .is_some_and(|property| is_secret_field(name, property))
        })
        .collect();
    let mut help = vec![
        "Fix the body (--describe shows its schema), or pass --no-validate to send it anyway."
            .to_owned(),
    ];
    if !missing_secrets.is_empty() {
        help.push(format!(
            "Secret fields ({}) are never flags: run in a terminal to be asked for them, or put them in --body @file or --body -.",
            missing_secrets.join(", ")
        ));
    }
    let lines: Vec<String> = problems
        .iter()
        .map(|problem| format!("  {problem}"))
        .collect();
    Err(CliError::InvalidBody {
        problems: lines.join("\n"),
        help: help.join("\n"),
    })
}

/// Who sent what, for the audit log and error help.
struct Sent<'a> {
    api: &'a ApiName,
    command: &'a CommandSpec,
    schemes: Vec<String>,
    sign_in: Option<SignIn>,
}

/// Sends the request, audits the outcome, and prints the response.
async fn send(
    plan: &RequestPlan,
    sent: &Sent<'_>,
    options: &RunOptions,
    paths: &Paths,
    columns: Option<&Format>,
) -> Result<(), CliError> {
    let response = send_response(plan, sent, options.timeout, paths).await?;
    // Preserve error bodies and status exit codes even when a success-only field is queried.
    let query = options.query.as_ref().filter(|_| response.is_success());
    print_response(&response, options.format, columns, query)?;
    if response.is_success() {
        if let Some(target) = &sent.sign_in {
            save_sign_in_token(&response, target, sent.api, paths).await?;
        }
        return Ok(());
    }
    Err(CliError::HttpStatus {
        help: rejection_help(response.status, !plan.credentials.is_empty(), sent.api),
        method: plan.method.to_string(),
        url: plan.url.clone(),
        status: response.status,
        reason: response.reason,
    })
}

/// Sends through the same authorization, transport and audit path as CLI calls.
/// The MCP adapter handles its own structured output and never saves login responses.
pub(crate) async fn agent_response(
    cached: &CachedCommands,
    command: &CommandSpec,
    mut plan: RequestPlan,
    timeout: Duration,
    paths: &Paths,
) -> Result<Response, CliError> {
    let api = ApiName::parse(&cached.api).map_err(|error| CliError::Internal(error.to_string()))?;
    let extensions = crate::plugins::for_api(paths, &api).await;
    let (credentials, schemes) =
        authorize(&api, command, cached, &plan, timeout, paths, &extensions).await?;
    plan.credentials = credentials;
    let sent = Sent {
        api: &api,
        command,
        schemes,
        sign_in: None,
    };
    send_response(&plan, &sent, timeout, paths).await
}

async fn send_response(
    plan: &RequestPlan,
    sent: &Sent<'_>,
    timeout: Duration,
    paths: &Paths,
) -> Result<Response, CliError> {
    let started = Instant::now();
    let label = format!("{} {}", plan.method, plan.url);
    let result =
        progress::with_spinner(label, async { HttpClient::new(timeout)?.send(plan).await }).await;
    let elapsed = started.elapsed();
    let audit_event = |status: Option<u16>, outcome: &'static str| AuditEvent::Request {
        api: sent.api.as_str(),
        command: sent.command.path.join(" "),
        method: plan.method.to_string(),
        url: &plan.url,
        status,
        outcome,
        duration_ms: elapsed.as_millis(),
        schemes: sent.schemes.clone(),
    };
    let response = match result {
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
    Ok(response)
}

/// Saves the token a successful sign-in returned, and says so on stderr so
/// stdout keeps only the response.
async fn save_sign_in_token(
    response: &Response,
    target: &SignIn,
    api: &ApiName,
    paths: &Paths,
) -> Result<(), CliError> {
    let Some(token) = response
        .json()
        .as_ref()
        .and_then(find_token)
        .map(str::to_owned)
    else {
        tracing::debug!(%api, "sign-in response has no token; nothing saved");
        return Ok(());
    };
    let credential = credentials::from_token(target.form, &token);
    let store = credentials::save(paths, api, &target.scheme, credential).await?;
    prompt::say(&format!(
        "Signed in: saved the token for `{api}` (`{}`, {store}). Pass --no-save to skip this.",
        target.scheme
    ))
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
    extensions: &[Plugin],
) -> Result<(Vec<CredentialPlacement>, Vec<String>), CliError> {
    if command.security.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut provided = provided_credentials(api, command, cached, extensions, timeout).await?;
    if let Some((variable, value)) = linked_credential(api, command, cached, paths).await? {
        // Environment and plugin providers still come first (ADR 0011).
        provided.entry(variable).or_insert(value);
    }
    let store = credentials::open(paths)?;
    let selection = {
        let (store, api) = (Arc::clone(&store), api.clone());
        let requirements = command.security.clone();
        let schemes = cached.commands.security_schemes.clone();
        credentials::blocking(move || {
            // Precedence (ADR 0004/0005): environment, then plugin provider, then store.
            let external = |name: &str| {
                std::env::var(name)
                    .ok()
                    .filter(|value| !value.is_empty())
                    .or_else(|| {
                        provided
                            .get(name)
                            .map(|value| value.expose_secret().to_owned())
                    })
            };
            select(&api, &requirements, &schemes, store.as_ref(), &external)
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

/// Runs plugin credential providers for this operation's schemes, keyed by
/// the environment variable they stand in for. Schemes already set in the
/// environment are skipped.
async fn provided_credentials(
    api: &ApiName,
    command: &CommandSpec,
    cached: &CachedCommands,
    extensions: &[Plugin],
    timeout: Duration,
) -> Result<HashMap<String, SecretString>, CliError> {
    let needed: BTreeSet<&str> = command
        .security
        .iter()
        .flat_map(|requirement| &requirement.schemes)
        .map(|scheme| scheme.scheme.as_str())
        .collect();
    let mut secrets = HashMap::new();
    for scheme in needed {
        let variable = environment_variable(api, scheme);
        if std::env::var(&variable).is_ok_and(|value| !value.is_empty()) {
            continue;
        }
        let Some(provider) = extensions
            .iter()
            .find_map(|plugin| plugin.credentials.get(scheme))
        else {
            continue;
        };
        let kind = cached
            .commands
            .security_schemes
            .get(scheme)
            .map_or("unknown", SecurityScheme::kind);
        let secret = fetch_credential(provider, api.as_str(), scheme, kind, timeout).await?;
        secrets.insert(variable, secret);
    }
    Ok(secrets)
}

/// The linked API's token for this API's token scheme (ADR 0011), keyed by
/// the environment variable it stands in for; `None` without a link, when the
/// operation doesn't use that scheme, or when the variable is set.
async fn linked_credential(
    api: &ApiName,
    command: &CommandSpec,
    cached: &CachedCommands,
    paths: &Paths,
) -> Result<Option<(String, SecretString)>, CliError> {
    let Some(source) = cached
        .credentials_from
        .as_deref()
        .and_then(|name| ApiName::parse(name).ok())
    else {
        return Ok(None);
    };
    let Some((scheme, form)) = token_scheme(&cached.commands.security_schemes) else {
        return Ok(None);
    };
    let uses_scheme = command
        .security
        .iter()
        .flat_map(|requirement| &requirement.schemes)
        .any(|requirement| requirement.scheme == scheme);
    let variable = environment_variable(api, &scheme);
    if !uses_scheme || environment_value(&variable).is_some() {
        return Ok(None);
    }
    let missing = || CliError::LinkedCredentialsMissing {
        api: api.to_string(),
        from: source.to_string(),
    };
    let source_cached = cache::load_or_build(paths, &source)
        .await?
        .ok_or_else(missing)?;
    let (source_scheme, _) =
        token_scheme(&source_cached.commands.security_schemes).ok_or_else(missing)?;
    let stored =
        if let Some(value) = environment_value(&environment_variable(&source, &source_scheme)) {
            Some(Credential::ApiKey {
                key: SecretString::from(value),
            })
        } else {
            let store = credentials::open(paths)?;
            let key = CredentialKey::new(&source, &source_scheme);
            credentials::blocking(move || store.get(&key).map_err(AuthError::from)).await?
        };
    let token = stored
        .as_ref()
        .and_then(credentials::bare_token)
        .ok_or_else(missing)?;
    Ok(Some((
        variable,
        credentials::as_scheme_value(form, token.expose_secret()),
    )))
}

fn environment_value(variable: &str) -> Option<String> {
    std::env::var(variable)
        .ok()
        .filter(|value| !value.is_empty())
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

/// Requests that change data need consent: a prompt in a terminal, `--yes`
/// elsewhere. A write without the path flags a sibling variant takes acts on
/// the whole collection, and the prompt says so.
fn confirm_change(plan: &RequestPlan, whole_collection: bool) -> Result<bool, CliError> {
    if !prompt::is_interactive() {
        return Err(CliError::SendConfirmationRequired {
            method: plan.method.to_string(),
            url: plan.url.clone(),
        });
    }
    let scope = if whole_collection { " (all items)" } else { "" };
    prompt::confirm(&format!("Send {} {}{scope}?", plan.method, plan.url))
}

/// JSON goes through `--output`; text and binary bodies are written as they came.
/// A plugin's columns, when given, replace the generic table for JSON responses.
fn print_response(
    response: &Response,
    format: OutputFormat,
    columns: Option<&Format>,
    query: Option<&output::ResponseQuery>,
) -> Result<(), CliError> {
    if let Some(json) = response.json() {
        let json = match query {
            Some(query) => query.select(&json)?,
            None => &json,
        };
        return match columns {
            Some(columns) => emit(format, json, |json| {
                let (headers, rows) = render_columns(json, columns);
                let headers: Vec<&str> = headers.iter().map(String::as_str).collect();
                output::table(&headers, &rows)
            }),
            None => emit(format, json, output::json_table),
        };
    }
    if query.is_some() {
        return Err(CliError::ResponseQuery(
            "the response is not JSON".to_owned(),
        ));
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
    /// The operations this node runs itself: its variants.
    variants: Vec<usize>,
}

impl Node {
    fn insert(&mut self, path: &[String], index: usize) {
        match path.split_first() {
            None => self.variants.push(index),
            Some((head, rest)) => self
                .children
                .entry(head.clone())
                .or_default()
                .insert(rest, index),
        }
    }

    /// A node can run its own variants and hold children (`countries` reads,
    /// `countries post` writes).
    fn build(&self, command: Command, api: &str, set: &CommandSet) -> Command {
        let mut command = if self.variants.is_empty() {
            command
                .about(children_summary(self.children.keys()))
                .subcommand_required(true)
                .arg_required_else_help(true)
        } else {
            let variants: Vec<&CommandSpec> = self
                .variants
                .iter()
                .map(|&index| &set.commands[index])
                .collect();
            let signs_in = sign_in_target(variants[0], &set.security_schemes).is_some();
            variant_command(command, api, &variants, signs_in)
                .subcommand_negates_reqs(true)
                .args_conflicts_with_subcommands(true)
        };
        if !self.children.is_empty() {
            // A resource may legitimately be called `help`.
            command = command.disable_help_subcommand(true);
        }
        self.children
            .iter()
            .fold(command, |command, (name, child)| {
                command.subcommand(child.build(Command::new(name.clone()), api, set))
            })
    }
}

/// `Commands: a, b, c`, cut short after a few so the parent's list stays readable.
fn children_summary<'a>(names: impl ExactSizeIterator<Item = &'a String>) -> String {
    let total = names.len();
    let shown: Vec<&str> = names.take(LISTED_CHILDREN).map(String::as_str).collect();
    if total > LISTED_CHILDREN {
        format!("Commands: {}, … ({total})", shown.join(", "))
    } else {
        format!("Commands: {}", shown.join(", "))
    }
}

/// One command for every variant: the union of their flags, with `required`
/// only enforced by clap when there is a single variant.
fn variant_command(
    command: Command,
    api: &str,
    variants: &[&CommandSpec],
    signs_in: bool,
) -> Command {
    let primary = variants
        .iter()
        .min_by_key(|variant| path_flags(variant).len())
        .copied()
        .unwrap_or(variants[0]);
    // Without a summary, the operation itself says what the command does.
    let summary = primary
        .summary
        .clone()
        .unwrap_or_else(|| operation_label(primary));
    let about = if primary.deprecated {
        format!("[deprecated] {summary}").trim_end().to_owned()
    } else {
        summary
    };
    let calls = if let [only] = variants {
        format!("Calls {}", operation_label(only))
    } else {
        let lines: Vec<String> = variants
            .iter()
            .map(|variant| format!("  {}", variant_line(variant)))
            .collect();
        format!(
            "Variants (chosen by the path flags you give):\n{}",
            lines.join("\n")
        )
    };
    let mut command = command
        .about(about)
        .after_help(format!("{calls}\n\n{}", examples(api, variants)));
    if variants.len() == 1
        && let Some(description) = &primary.description
    {
        command = command.long_about(description.clone());
    }
    command = merged_arguments(variants)
        .iter()
        .fold(command, |command, argument| {
            command.arg(clap_argument(argument))
        });
    command = command
        .arg(
            Arg::new(DESCRIBE)
                .long(DESCRIBE)
                .exclusive(true)
                .action(ArgAction::SetTrue)
                .help("Print this command's flags and body schema as JSON, then exit"),
        )
        .arg(
            Arg::new(DRY_RUN)
                .long(DRY_RUN)
                .action(ArgAction::SetTrue)
                .help("Print the HTTP request instead of sending it"),
        )
        .arg(
            Arg::new(INTERACTIVE)
                .long(INTERACTIVE)
                .short('i')
                .action(ArgAction::SetTrue)
                .help("Ask for every flag and body field in turn (needs a terminal)"),
        );
    // Variants share a method (naming rejects anything else).
    if primary.method != HttpMethod::Get {
        command = command.arg(
            Arg::new(YES)
                .long(YES)
                .action(ArgAction::SetTrue)
                .help("Send without asking for confirmation (required when not in a terminal)"),
        );
    }
    if signs_in {
        command = command.arg(
            Arg::new(NO_SAVE)
                .long(NO_SAVE)
                .action(ArgAction::SetTrue)
                .help("Don't save the token this sign-in returns as the API's credential"),
        );
    }
    if let Some(body) = variants.iter().find_map(|variant| variant.body.as_ref()) {
        command = command
            .arg(
                Arg::new(BODY)
                    .long(BODY)
                    .value_name("JSON|@FILE|-")
                    .help(format!(
                        "Request body ({}): inline, @file, or - for stdin",
                        body.content_type
                    )),
            )
            .arg(
                Arg::new(NO_VALIDATE)
                    .long(NO_VALIDATE)
                    .action(ArgAction::SetTrue)
                    .help("Send the body without checking it against the specification"),
            );
    }
    command
}

/// Each flag once, in first-seen order. Variants that disagree on a flag's
/// type or allowed values leave it a plain string; values are checked again
/// against the chosen variant when the request is planned.
fn merged_arguments(variants: &[&CommandSpec]) -> Vec<ArgumentSpec> {
    let single = variants.len() == 1;
    let mut merged: Vec<ArgumentSpec> = Vec::new();
    for argument in variants.iter().flat_map(|variant| &variant.arguments) {
        match merged.iter_mut().find(|seen| seen.name == argument.name) {
            Some(seen) => {
                if seen.value_type != argument.value_type {
                    seen.value_type = ValueType::String;
                }
                if seen.allowed_values != argument.allowed_values {
                    seen.allowed_values.clear();
                }
                seen.multiple |= argument.multiple;
            }
            None => merged.push(ArgumentSpec {
                required: single && argument.required,
                ..argument.clone()
            }),
        }
    }
    merged
}

/// Examples built from the command's own variants.
fn examples(api: &str, variants: &[&CommandSpec]) -> String {
    let base = std::iter::once(format!("opencli {api}"))
        .chain(variants[0].path.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    let mut lines: Vec<(String, String)> = Vec::new();
    for variant in variants {
        let flags: Vec<String> = variant
            .arguments
            .iter()
            .filter(|argument| matches!(argument.target, ArgumentTarget::Path(_)))
            .map(flag_usage)
            .collect();
        let call = std::iter::once(base.clone())
            .chain(flags)
            .collect::<Vec<_>>()
            .join(" ");
        lines.push((call.clone(), operation_label(variant)));
        if variant.body.is_some() {
            lines.push((
                format!("{call} --body @body.json"),
                "the body from a file".to_owned(),
            ));
        }
    }
    lines.push((
        format!("{base} -i"),
        "ask for every flag and body field".to_owned(),
    ));
    lines.push((
        format!("{base} --dry-run"),
        "show the request without sending it".to_owned(),
    ));
    lines.push((
        format!("{base} --describe"),
        "flags and body schema as JSON".to_owned(),
    ));
    let width = lines
        .iter()
        .map(|(command, _)| command.len())
        .max()
        .unwrap_or(0);
    let lines: Vec<String> = lines
        .iter()
        .map(|(command, note)| format!("  {command:<width$}   # {note}"))
        .collect();
    format!("Examples:\n{}", lines.join("\n"))
}

fn clap_argument(argument: &ArgumentSpec) -> Arg {
    let help = argument
        .help
        .clone()
        .unwrap_or_else(|| default_help(&argument.target));
    Arg::new(argument.name.clone())
        .long(argument.name.clone())
        .value_parser(value_parser(argument))
        .help(help)
        // `--describe` is exclusive, which lifts `required` while it is present.
        // In a terminal, missing required values are asked for instead (fill.rs).
        .required(argument.required && !prompt::is_interactive())
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
        ValueType::File => "file",
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
    // Form and multipart bodies take a JSON object of fields; forms also an encoded string.
    if (is_form_media_type(&spec.content_type) || is_multipart_media_type(&spec.content_type))
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
        lines.extend(
            body.files
                .iter()
                .map(|file| format!("Upload: {} ← {}", file.field, file.path)),
        );
        lines.push(String::new());
        lines.push(match &body.value {
            Value::String(text) => text.clone(),
            value => serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
        });
    }
    lines.join("\n")
}

/// `--describe` output: everything an agent needs to call the command, one
/// entry per variant (ADR 0009).
#[derive(Serialize)]
struct CommandContract<'a> {
    command: String,
    servers: &'a [String],
    variants: Vec<VariantContract<'a>>,
}

#[derive(Serialize)]
struct VariantContract<'a> {
    method: HttpMethod,
    path: &'a str,
    operation_id: Option<&'a str>,
    summary: Option<&'a str>,
    description: Option<&'a str>,
    deprecated: bool,
    arguments: Vec<DescribedArgument<'a>>,
    body: Option<&'a BodySpec>,
}

#[derive(Serialize)]
struct DescribedArgument<'a> {
    /// How to pass it: `--flag`.
    usage: String,
    #[serde(flatten)]
    spec: &'a ArgumentSpec,
}

impl<'a> CommandContract<'a> {
    fn new(cached: &'a CachedCommands, command: String, variants: &[&'a CommandSpec]) -> Self {
        let variants = variants
            .iter()
            .map(|variant| VariantContract {
                method: variant.method,
                path: &variant.path_template,
                operation_id: variant.operation_id.as_deref(),
                summary: variant.summary.as_deref(),
                description: variant.description.as_deref(),
                deprecated: variant.deprecated,
                arguments: variant
                    .arguments
                    .iter()
                    .map(|spec| DescribedArgument {
                        usage: format!("--{}", spec.name),
                        spec,
                    })
                    .collect(),
                body: variant.body.as_ref(),
            })
            .collect();
        Self {
            command,
            servers: &cached.commands.servers,
            variants,
        }
    }
}
