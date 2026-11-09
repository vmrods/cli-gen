//! `opencli auth …`: storing, inspecting and removing credentials (ADR 0004).

use std::collections::BTreeMap;
use std::io::Write as _;
use std::time::Duration;

use opencli_auth::{
    AuthError, CallbackListener, Credential, CredentialKey, GrantFlow, OAuth2Credential, Pkce,
    TokenClient, TokenResponse, authorization_url, environment_variable, new_state,
};
use opencli_core::{ApiName, OAuth2Flow, SecurityScheme};
use secrecy::SecretString;
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::io::AsyncReadExt;

use crate::audit::{self, AuditEvent};
use crate::cache;
use crate::cli::{AuthCommand, FlowArg, LoginArgs, OutputFormat};
use crate::credentials::{self, blocking};
use crate::error::{CliError, StoreFailure};
use crate::output::{emit, table};
use crate::paths::Paths;
use crate::{browser, dynamic, plugins, progress, prompt};

/// How long to wait for the browser to come back from the sign-in page.
const SIGN_IN_TIMEOUT: Duration = Duration::from_mins(5);

pub async fn run(
    command: AuthCommand,
    format: OutputFormat,
    paths: &Paths,
    timeout: Duration,
) -> Result<(), CliError> {
    match command {
        AuthCommand::Login(args) => login(args, format, paths, timeout).await,
        AuthCommand::Logout { name, scheme } => {
            logout(&name, scheme.as_deref(), format, paths).await
        }
        AuthCommand::Status { name } => status(&name, format, paths).await,
    }
}

async fn schemes_of(
    paths: &Paths,
    api: &ApiName,
) -> Result<BTreeMap<String, SecurityScheme>, CliError> {
    cache::load_or_build(paths, api)
        .await?
        .map(|cached| cached.commands.security_schemes)
        .ok_or_else(|| StoreFailure::not_found(api).into())
}

#[derive(Serialize)]
struct LoginResult<'a> {
    api: &'a str,
    scheme: &'a str,
    store: &'static str,
}

async fn login(
    args: LoginArgs,
    format: OutputFormat,
    paths: &Paths,
    timeout: Duration,
) -> Result<(), CliError> {
    let cached = cache::load_or_build(paths, &args.name)
        .await?
        .ok_or_else(|| StoreFailure::not_found(&args.name))?;
    let schemes = &cached.commands.security_schemes;
    let (scheme_name, scheme) = choose_scheme(&args.name, schemes, args.scheme.as_deref())?;
    // The API's own sign-in, when it has one for this scheme, saves typing a token.
    if !args.with_token
        && prompt::is_interactive()
        && let Some((command, target)) = dynamic::best_sign_in(&cached, scheme_name)
        && prompt::confirm_default_yes(&format!(
            "Sign in with your account ({} {})?",
            command.method, command.path_template
        ))?
    {
        let store_name = dynamic::sign_in(&cached, command, &target, timeout, paths).await?;
        return emit_login(format, &args.name, scheme_name, store_name);
    }
    let credential = match scheme {
        SecurityScheme::ApiKey { name, .. } => Credential::ApiKey {
            key: read_secret(&format!("API key ({name}): "), args.with_token).await?,
        },
        SecurityScheme::HttpBearer { .. } | SecurityScheme::OpenIdConnect { .. } => {
            Credential::Bearer {
                token: read_secret("Bearer token: ", args.with_token).await?,
            }
        }
        SecurityScheme::HttpBasic => {
            let username = value_or_prompt(
                args.username.clone(),
                "Username: ",
                "a username",
                "--username",
            )?;
            Credential::Basic {
                username,
                password: read_secret("Password: ", args.with_token).await?,
            }
        }
        SecurityScheme::OAuth2 { flows } => {
            Credential::OAuth2(oauth_login(&args, scheme_name, flows, timeout).await?)
        }
        SecurityScheme::Unsupported { description } => {
            return Err(AuthError::UnsupportedScheme {
                scheme: scheme_name.clone(),
                description: description.clone(),
            }
            .into());
        }
    };

    let store_name = credentials::save(paths, &args.name, scheme_name, credential).await?;
    emit_login(format, &args.name, scheme_name, store_name)
}

fn emit_login(
    format: OutputFormat,
    api: &ApiName,
    scheme: &str,
    store: &'static str,
) -> Result<(), CliError> {
    let result = LoginResult {
        api: api.as_str(),
        scheme,
        store,
    };
    emit(format, &result, |result| {
        format!(
            "Logged in to `{}` with `{}` (stored in the {}).",
            result.api, result.scheme, result.store
        )
    })
}

fn choose_scheme<'a>(
    api: &ApiName,
    schemes: &'a BTreeMap<String, SecurityScheme>,
    requested: Option<&str>,
) -> Result<(&'a String, &'a SecurityScheme), CliError> {
    if let Some(requested) = requested {
        return schemes
            .get_key_value(requested)
            .ok_or_else(|| CliError::UnknownScheme {
                api: api.to_string(),
                scheme: requested.to_owned(),
                available: schemes.keys().cloned().collect(),
            });
    }
    let supported: Vec<(&String, &SecurityScheme)> = schemes
        .iter()
        .filter(|(_, scheme)| !matches!(scheme, SecurityScheme::Unsupported { .. }))
        .collect();
    match supported.as_slice() {
        [] => Err(CliError::NoSupportedScheme {
            api: api.to_string(),
        }),
        [only] => Ok(*only),
        many if prompt::is_interactive() => {
            let labels: Vec<String> = many
                .iter()
                .map(|(name, scheme)| format!("{name} ({})", scheme.kind()))
                .collect();
            Ok(many[prompt::choose("Security scheme", &labels)?])
        }
        many => Err(CliError::SchemeRequired {
            api: api.to_string(),
            schemes: many.iter().map(|(name, _)| (*name).clone()).collect(),
        }),
    }
}

async fn oauth_login(
    args: &LoginArgs,
    scheme_name: &str,
    flows: &[OAuth2Flow],
    timeout: Duration,
) -> Result<OAuth2Credential, CliError> {
    let flow = args.flow.or_else(|| {
        flows.first().map(|flow| match flow {
            OAuth2Flow::ClientCredentials { .. } => FlowArg::ClientCredentials,
            OAuth2Flow::AuthorizationCode { .. } => FlowArg::AuthorizationCode,
        })
    });
    let unsupported = |flow: &str| CliError::UnsupportedFlow {
        scheme: scheme_name.to_owned(),
        flow: flow.to_owned(),
    };
    let tokens = TokenClient::new(timeout)?;
    match flow {
        Some(FlowArg::ClientCredentials) => {
            let declared = flows.iter().find_map(|flow| match flow {
                OAuth2Flow::ClientCredentials {
                    token_url, scopes, ..
                } => Some((token_url, scopes)),
                OAuth2Flow::AuthorizationCode { .. } => None,
            });
            let (token_url, scopes) = declared.ok_or_else(|| unsupported("client-credentials"))?;
            client_credentials_login(args, &tokens, token_url, scopes).await
        }
        Some(FlowArg::AuthorizationCode) => {
            let declared = flows.iter().find_map(|flow| match flow {
                OAuth2Flow::AuthorizationCode {
                    authorization_url,
                    token_url,
                    scopes,
                    ..
                } => Some((authorization_url, token_url, scopes)),
                OAuth2Flow::ClientCredentials { .. } => None,
            });
            let (authorize_url, token_url, scopes) =
                declared.ok_or_else(|| unsupported("authorization-code"))?;
            authorization_code_login(args, &tokens, authorize_url, token_url, scopes).await
        }
        None => Err(unsupported("supported")),
    }
}

fn client_id(args: &LoginArgs) -> Result<String, CliError> {
    value_or_prompt(
        args.client_id.clone(),
        "Client ID: ",
        "an OAuth client ID",
        "--client-id",
    )
}

async fn client_credentials_login(
    args: &LoginArgs,
    tokens: &TokenClient,
    token_url: &str,
    declared_scopes: &BTreeMap<String, String>,
) -> Result<OAuth2Credential, CliError> {
    let client_id = client_id(args)?;
    let secret = read_secret("Client secret: ", args.with_token).await?;
    let scopes = requested_scopes(&args.scopes, declared_scopes);
    let response = progress::with_spinner(
        "Requesting a token",
        tokens.client_credentials(token_url, &client_id, &secret, &scopes),
    )
    .await?;
    Ok(oauth_credential(
        GrantFlow::ClientCredentials,
        client_id,
        Some(secret),
        scopes,
        response,
    ))
}

/// Browser sign-in with PKCE and a loopback redirect (RFC 8252).
async fn authorization_code_login(
    args: &LoginArgs,
    tokens: &TokenClient,
    authorize_url: &str,
    token_url: &str,
    declared_scopes: &BTreeMap<String, String>,
) -> Result<OAuth2Credential, CliError> {
    let client_id = client_id(args)?;
    let secret = optional_client_secret(args.with_token).await?;
    let scopes = requested_scopes(&args.scopes, declared_scopes);
    let listener = CallbackListener::bind(args.redirect_port).await?;
    let redirect_uri = listener.redirect_uri();
    let (state, pkce) = (new_state()?, Pkce::generate()?);
    let url = authorization_url(
        authorize_url,
        &client_id,
        &redirect_uri,
        &scopes,
        &state,
        &pkce,
    )?;
    writeln!(
        std::io::stderr(),
        "Open this URL to sign in to `{}`:\n\n  {url}\n\nWaiting for the redirect to {redirect_uri} …",
        args.name
    )
    .map_err(CliError::Terminal)?;
    if !args.no_browser {
        browser::open(url.as_str());
    }
    let code = listener.wait_for_code(&state, SIGN_IN_TIMEOUT).await?;
    let response = tokens
        .exchange_code(
            token_url,
            &client_id,
            secret.as_ref(),
            &code,
            &pkce,
            &redirect_uri,
        )
        .await?;
    Ok(oauth_credential(
        GrantFlow::AuthorizationCode,
        client_id,
        secret,
        scopes,
        response,
    ))
}

fn oauth_credential(
    flow: GrantFlow,
    client_id: String,
    client_secret: Option<SecretString>,
    scopes: Vec<String>,
    response: TokenResponse,
) -> OAuth2Credential {
    OAuth2Credential {
        flow,
        client_id,
        client_secret,
        scopes,
        expires_at: response.expires_at(OffsetDateTime::now_utc()),
        access_token: response.access_token,
        refresh_token: response.refresh_token,
    }
}

fn requested_scopes(requested: &[String], declared: &BTreeMap<String, String>) -> Vec<String> {
    if requested.is_empty() {
        declared.keys().cloned().collect()
    } else {
        requested.to_vec()
    }
}

/// Secrets come from stdin (`--with-token`) or a hidden prompt, never argv,
/// where other users could read them from the process list.
async fn read_secret(question: &str, from_stdin: bool) -> Result<SecretString, CliError> {
    let secret = if from_stdin {
        read_stdin_secret().await?
    } else if prompt::is_interactive() {
        let question = question.to_owned();
        tokio::task::spawn_blocking(move || prompt::ask_secret(&question))
            .await
            .map_err(|error| CliError::Internal(error.to_string()))??
    } else {
        return Err(CliError::LoginInputRequired {
            what: "the secret".to_owned(),
            flag: "--with-token and the secret on stdin".to_owned(),
        });
    };
    if secret.is_empty() {
        return Err(CliError::LoginInputRequired {
            what: "a non-empty secret".to_owned(),
            flag: "the secret on stdin with --with-token".to_owned(),
        });
    }
    Ok(SecretString::from(secret))
}

async fn read_stdin_secret() -> Result<String, CliError> {
    let mut text = String::new();
    tokio::io::stdin()
        .read_to_string(&mut text)
        .await
        .map_err(CliError::Terminal)?;
    Ok(text.trim_end_matches(['\r', '\n']).to_owned())
}

/// Public clients (PKCE without a secret) are common for authorization code.
async fn optional_client_secret(from_stdin: bool) -> Result<Option<SecretString>, CliError> {
    let secret = if from_stdin {
        read_stdin_secret().await?
    } else if prompt::is_interactive() {
        tokio::task::spawn_blocking(|| {
            prompt::ask_secret("Client secret (leave empty for a public client): ")
        })
        .await
        .map_err(|error| CliError::Internal(error.to_string()))??
    } else {
        String::new()
    };
    Ok((!secret.is_empty()).then(|| SecretString::from(secret)))
}

fn value_or_prompt(
    given: Option<String>,
    question: &str,
    what: &str,
    flag: &str,
) -> Result<String, CliError> {
    if let Some(value) = given.filter(|value| !value.is_empty()) {
        return Ok(value);
    }
    let required = || CliError::LoginInputRequired {
        what: what.to_owned(),
        flag: flag.to_owned(),
    };
    if !prompt::is_interactive() {
        return Err(required());
    }
    Some(prompt::ask(question)?)
        .filter(|value| !value.is_empty())
        .ok_or_else(required)
}

#[derive(Serialize)]
struct LogoutResult<'a> {
    api: &'a str,
    removed: Vec<String>,
}

async fn logout(
    api: &ApiName,
    scheme: Option<&str>,
    format: OutputFormat,
    paths: &Paths,
) -> Result<(), CliError> {
    let schemes = schemes_of(paths, api).await?;
    let targets: Vec<String> = match scheme {
        Some(scheme) => vec![choose_scheme(api, &schemes, Some(scheme))?.0.clone()],
        None => schemes.keys().cloned().collect(),
    };
    let store = credentials::open(paths)?;
    let owner = api.clone();
    let removed = blocking(move || {
        let mut removed = Vec::new();
        for scheme in targets {
            if store.delete(&CredentialKey::new(&owner, &scheme))? {
                removed.push(scheme);
            }
        }
        Ok(removed)
    })
    .await?;
    let event = AuditEvent::Logout {
        api: api.as_str(),
        schemes: &removed,
    };
    audit::record(paths, &event).await;
    let result = LogoutResult {
        api: api.as_str(),
        removed,
    };
    emit(format, &result, |result| {
        if result.removed.is_empty() {
            format!("No stored credentials for `{}`.", result.api)
        } else {
            format!(
                "Removed credentials for `{}`: {}.",
                result.api,
                result.removed.join(", ")
            )
        }
    })
}

#[derive(Serialize)]
struct SchemeStatus {
    scheme: String,
    kind: &'static str,
    /// `environment`, the store's name, or null when not configured.
    configured: Option<String>,
    environment_variable: String,
    expires_at: Option<String>,
}

async fn status(api: &ApiName, format: OutputFormat, paths: &Paths) -> Result<(), CliError> {
    let schemes = schemes_of(paths, api).await?;
    // Providers are reported, not run: status must not execute plugin programs.
    let providers: BTreeMap<String, String> = plugins::for_api(paths, api)
        .await
        .into_iter()
        .flat_map(|plugin| {
            let name = plugin.name.to_string();
            plugin
                .credentials
                .into_keys()
                .map(move |scheme| (scheme, format!("plugin {name}")))
        })
        .collect();
    let store = credentials::open(paths)?;
    let owner = api.clone();
    let statuses = blocking(move || {
        let mut statuses = Vec::new();
        for (name, scheme) in &schemes {
            let variable = environment_variable(&owner, name);
            let (configured, expires_at) =
                if std::env::var(&variable).is_ok_and(|value| !value.is_empty()) {
                    (Some("environment".to_owned()), None)
                } else if let Some(plugin) = providers.get(name) {
                    (Some(plugin.clone()), None)
                } else {
                    match store.get(&CredentialKey::new(&owner, name))? {
                        Some(Credential::OAuth2(token)) => (
                            Some(store.name().to_owned()),
                            token.expires_at.and_then(|at| at.format(&Rfc3339).ok()),
                        ),
                        Some(_) => (Some(store.name().to_owned()), None),
                        None => (None, None),
                    }
                };
            statuses.push(SchemeStatus {
                scheme: name.clone(),
                kind: scheme.kind(),
                configured,
                environment_variable: variable,
                expires_at,
            });
        }
        Ok(statuses)
    })
    .await?;
    emit(format, &statuses, |statuses: &Vec<SchemeStatus>| {
        let rows: Vec<Vec<String>> = statuses
            .iter()
            .map(|status| {
                vec![
                    status.scheme.clone(),
                    status.kind.to_owned(),
                    status.configured.clone().unwrap_or_else(|| "-".to_owned()),
                    status.expires_at.clone().unwrap_or_default(),
                    status.environment_variable.clone(),
                ]
            })
            .collect();
        table(
            &[
                "SCHEME",
                "TYPE",
                "CONFIGURED",
                "EXPIRES",
                "ENVIRONMENT VARIABLE",
            ],
            &rows,
        )
    })
}
