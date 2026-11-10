use opencli_core::{ApiName, CommandSpec, HttpMethod, ValueType, is_secret_field, sign_in_target};
use opencli_runtime::{RequestPlan, plan_request, typed_value};
use serde_json::{Value, json};

use super::{ApiServer, Failure, MAX_PAGE_ITEMS, Operation, Request, Search, parse};
use crate::cache::{self, CachedCommands};
use crate::error::CliError;
use crate::output::ResponseQuery;
use crate::{dynamic, server};

impl ApiServer {
    pub(super) async fn call(&self, name: &str, arguments: Value) -> Result<Value, Failure> {
        match name {
            "list_apis" => {
                let _: std::collections::BTreeMap<String, Value> = parse(arguments.clone())?;
                if arguments
                    .as_object()
                    .is_some_and(|object| !object.is_empty())
                {
                    return Err(Failure::new(
                        "invalid_arguments",
                        "list_apis takes no arguments",
                    ));
                }
                let mut apis = Vec::new();
                for api in &self.apis {
                    let cached = self.load(api.as_str()).await?;
                    apis.push(
                        json!({"name": cached.api, "title": cached.title, "version": cached.version,
                        "operation_count": cached.commands.commands.iter().filter(|command| self.permits(command)).count()}),
                    );
                }
                Ok(json!({"apis": apis}))
            }
            "search_operations" => self.search(parse(arguments)?).await,
            "describe_operation" => {
                let request: Operation = parse(arguments)?;
                let cached = self.load(&request.api).await?;
                let command = self.operation(&cached, &request.operation)?;
                Ok(
                    json!({"api": cached.api, "operation": identifier(command), "contract": command}),
                )
            }
            "plan_request" | "execute_operation" => {
                self.request(parse(arguments)?, name == "execute_operation")
                    .await
            }
            _ => Err(Failure::new("unknown_tool", "Unknown tool; use tools/list")),
        }
    }

    async fn load(&self, name: &str) -> Result<CachedCommands, Failure> {
        let api = ApiName::parse(name)
            .map_err(|_| Failure::new("api_not_allowed", "API is not exposed by this server"))?;
        if !self.apis.contains(&api) {
            return Err(Failure::new(
                "api_not_allowed",
                "API is not exposed by this server",
            ));
        }
        cache::load_or_build(&self.paths, &api)
            .await
            .map_err(failure)?
            .ok_or_else(|| {
                Failure::new("api_unavailable", "The exposed API is no longer registered")
            })
    }

    fn permits(&self, command: &CommandSpec) -> bool {
        self.operations.is_empty() || self.operations.contains(&identifier(command))
    }

    fn operation<'a>(
        &self,
        cached: &'a CachedCommands,
        operation: &str,
    ) -> Result<&'a CommandSpec, Failure> {
        cached
            .commands
            .commands
            .iter()
            .find(|command| identifier(command) == operation && self.permits(command))
            .ok_or_else(|| {
                Failure::new(
                    "operation_not_found",
                    "Use an exact allowed operation identifier returned by search_operations",
                )
            })
    }

    async fn search(&self, request: Search) -> Result<Value, Failure> {
        check_page(request.limit)?;
        let cached = self.load(&request.api).await?;
        let words: Vec<String> = request
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let matches: Vec<&CommandSpec> = cached
            .commands
            .commands
            .iter()
            .filter(|command| self.permits(command))
            .filter(|command| {
                let text = format!(
                    "{} {} {} {} {}",
                    command.path.join(" "),
                    identifier(command),
                    command.summary.as_deref().unwrap_or_default(),
                    command.description.as_deref().unwrap_or_default(),
                    command.operation_id.as_deref().unwrap_or_default()
                )
                .to_lowercase();
                words.iter().all(|word| text.contains(word))
            })
            .collect();
        let operations: Vec<Value> = matches.iter().skip(request.offset).take(request.limit).map(|command| json!({
            "operation": identifier(command), "command": command.path, "summary": command.summary,
            "method": command.method, "path": command.path_template,
        })).collect();
        Ok(page(
            "operations",
            operations,
            request.offset,
            matches.len(),
        ))
    }

    async fn request(&self, request: Request, execute: bool) -> Result<Value, Failure> {
        check_page(request.limit)?;
        let query: Option<ResponseQuery> = request
            .query
            .as_deref()
            .map(str::parse)
            .transpose()
            .map_err(|message| Failure::new("invalid_query", message))?;
        let cached = self.load(&request.api).await?;
        let command = self.operation(&cached, &request.operation)?;
        if execute
            && command.method != HttpMethod::Get
            && (!self.allow_writes || !request.confirmed)
        {
            return Err(Failure::new(
                "write_not_allowed",
                "Writes need --allow-writes at server startup and confirmed=true for this authorized request",
            ));
        }
        if execute && command.method != HttpMethod::Get && request.offset != 0 {
            return Err(Failure::new(
                "write_paging_not_allowed",
                "Use a read operation to inspect further results; do not replay a write for another output page",
            ));
        }
        if execute && sign_in_target(command, &cached.commands.security_schemes).is_some() {
            return Err(Failure::new(
                "login_requires_cli",
                "Sign in with opencli auth login outside MCP; this server does not run sign-in operations",
            ));
        }
        let plan = prepare(&cached, command, &request)?;
        if !execute {
            return Ok(json!({"plan": preview(&plan)?}));
        }
        let response = dynamic::agent_response(&cached, command, plan, self.timeout, &self.paths)
            .await
            .map_err(failure)?;
        let Some(mut data) = response
            .json()
            .or_else(|| response.body.is_empty().then_some(Value::Null))
        else {
            return Ok(json!({"http_status": response.status, "request_sent": true,
                "error": Failure::new("non_json_response", "The API returned a non-JSON response. Use the CLI for text/binary reads; do not resend a successful write.")}));
        };
        // Scrub before selection, so selecting data.token cannot bypass name-based redaction.
        redact(&mut data);
        if response.is_success()
            && let Some(query) = query
        {
            data = match query.select(&data) {
                Ok(selected) => selected.clone(),
                Err(error) => {
                    return Ok(
                        json!({"http_status": response.status, "request_sent": true, "error": failure(error)}),
                    );
                }
            };
        }
        let mut result = if let Value::Array(items) = data {
            let total = items.len();
            page(
                "data",
                items
                    .into_iter()
                    .skip(request.offset)
                    .take(request.limit)
                    .collect(),
                request.offset,
                total,
            )
        } else {
            json!({"data": data, "truncated": false})
        };
        result["http_status"] = json!(response.status);
        result["request_sent"] = json!(true);
        if command.method != HttpMethod::Get && result["truncated"] == true {
            result["next_offset"] = Value::Null;
            result["paging_note"] =
                json!("Use a read operation for further results; do not replay this write.");
        }
        if !response.is_success() {
            result["error"] = json!(Failure::new(
                "http_error",
                format!(
                    "API returned HTTP {}; inspect the response and do not retry writes automatically",
                    response.status
                )
            ));
        }
        Ok(result)
    }
}

pub(super) fn identifier(command: &CommandSpec) -> String {
    format!("{} {}", command.method, command.path_template)
}

fn check_page(limit: usize) -> Result<(), Failure> {
    if !(1..=MAX_PAGE_ITEMS).contains(&limit) {
        return Err(Failure::new(
            "invalid_arguments",
            format!("limit must be between 1 and {MAX_PAGE_ITEMS}"),
        ));
    }
    Ok(())
}

fn page(key: &str, items: Vec<Value>, offset: usize, total: usize) -> Value {
    let returned = items.len();
    let next = offset.saturating_add(returned);
    let mut value = json!({"offset": offset, "total_items": total, "returned_items": returned,
        "truncated": offset > 0 || next < total, "next_offset": (next < total).then_some(next)});
    value[key] = Value::Array(items);
    value
}

fn prepare(
    cached: &CachedCommands,
    command: &CommandSpec,
    request: &Request,
) -> Result<RequestPlan, Failure> {
    for name in request.arguments.keys() {
        if !command
            .arguments
            .iter()
            .any(|argument| &argument.name == name)
        {
            return Err(Failure::new(
                "invalid_arguments",
                format!("Unknown argument `{name}` for this operation"),
            ));
        }
    }
    if request.body.is_some() && command.body.is_none() {
        return Err(Failure::new(
            "invalid_arguments",
            "This operation does not accept a body",
        ));
    }
    let mut values = Vec::new();
    for argument in &command.arguments {
        let Some(value) = request.arguments.get(&argument.name) else {
            if argument.required {
                return Err(Failure::new(
                    "missing_argument",
                    format!("Missing `{}`", argument.name),
                ));
            }
            values.push(None);
            continue;
        };
        // File flags would let a model send arbitrary local files to a server.
        if argument.value_type == ValueType::File {
            return Err(Failure::new(
                "local_file_not_allowed",
                "Use the CLI explicitly for file uploads",
            ));
        }
        let items = match value {
            Value::Array(items) if argument.multiple && !items.is_empty() => items.iter().collect(),
            Value::Array(_) | Value::Object(_) | Value::Null => {
                return Err(Failure::new(
                    "invalid_arguments",
                    "Arguments must be scalars or nonempty arrays for repeatable flags",
                ));
            }
            item => vec![item],
        };
        let mut raw = Vec::new();
        for item in items {
            if !matches!(item, Value::String(_) | Value::Bool(_) | Value::Number(_)) {
                return Err(Failure::new(
                    "invalid_arguments",
                    "Array arguments must contain scalar values",
                ));
            }
            let text = item
                .as_str()
                .map_or_else(|| item.to_string(), str::to_owned);
            if typed_value(&argument.name, &text, argument.value_type).is_err()
                || (!argument.allowed_values.is_empty() && !argument.allowed_values.contains(&text))
            {
                return Err(Failure::new(
                    "invalid_arguments",
                    format!("Invalid type or enum choice for `{}`", argument.name),
                ));
            }
            raw.push(text);
        }
        values.push(Some(raw));
    }
    let base = server::resolve(
        None,
        cached.base_url.as_deref(),
        &cached.commands.servers,
        &cached.source,
    )
    .map_err(failure)?;
    let plan = plan_request(command, Some(&base), &values, request.body.clone()).map_err(|_| {
        Failure::new(
            "invalid_request",
            "Request planning failed; check required body and path arguments",
        )
    })?;
    dynamic::check_body(command, &plan).map_err(failure)?;
    Ok(plan)
}

fn preview(plan: &RequestPlan) -> Result<Value, Failure> {
    let mut value = serde_json::to_value(plan)
        .map_err(|_| Failure::new("internal_error", "Unable to serialize request plan"))?;
    // Header/cookie values are unnecessary for discovery and can carry arbitrary secrets.
    for key in ["headers", "cookies", "query"] {
        if let Some(pairs) = value[key].as_array_mut() {
            for pair in pairs {
                if key != "query" || pair[0].as_str().is_some_and(secret_name) {
                    pair[1] = json!("<redacted>");
                }
            }
        }
    }
    redact(&mut value);
    value["credentials"] = json!([]);
    Ok(value)
}

fn secret_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "cookie" | "set-cookie" | "credentials"
    ) || is_secret_field(name, &json!({"type": "string"}))
}

fn redact(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (name, value) in fields {
                if secret_name(name) {
                    *value = json!("<redacted>");
                } else {
                    redact(value);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact),
        _ => {}
    }
}

fn failure(error: CliError) -> Failure {
    let code = match error {
        CliError::Auth(_)
        | CliError::LinkedCredentialsMissing { .. }
        | CliError::KeyringUnavailable => "authentication_required",
        CliError::Timeout { .. } => "timeout",
        CliError::Network(_) => "network_error",
        CliError::ResponseQuery(_) => "invalid_query",
        CliError::InvalidBody { .. } => "invalid_body",
        _ => "request_failed",
    };
    // Provider stderr and store errors may contain external text; keep it off the model channel.
    let message = match error {
        CliError::InvalidBody { problems, .. } => problems,
        CliError::ResponseQuery(message) => message,
        _ => "Request failed; inspect the API configuration and authenticate with opencli auth login outside MCP".into(),
    };
    Failure::new(code, message)
}
