//! Picks credentials for an operation and turns them into request placements.
//!
//! Split in two so blocking store access (keyring, Argon2) can run on a
//! blocking thread: [`select`] reads, [`place`] may refresh tokens over HTTP.

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use opencli_core::{ApiKeyLocation, ApiName, OAuth2Flow, SecurityRequirement, SecurityScheme};
use opencli_runtime::{CredentialLocation, CredentialPlacement};
use secrecy::{ExposeSecret, SecretString};
use time::OffsetDateTime;

use crate::oauth::TokenClient;
use crate::{AuthError, Credential, CredentialKey, CredentialStore, GrantFlow, OAuth2Credential};

/// Tokens this close to expiry are refreshed before use.
const EXPIRY_SKEW: time::Duration = time::Duration::seconds(60);
const AUTHORIZATION: &str = "Authorization";
const ENVIRONMENT_PREFIX: &str = "OPENCLI";

/// Credentials chosen for one call; empty means send anonymously.
#[derive(Debug, Default)]
pub struct Selection {
    pub items: Vec<Selected>,
}

#[derive(Debug)]
pub struct Selected {
    pub key: CredentialKey,
    pub scheme_name: String,
    pub scheme: SecurityScheme,
    pub source: CredentialSource,
}

#[derive(Debug)]
pub enum CredentialSource {
    /// From `OPENCLI_<API>_<SCHEME>`, used as-is and never stored.
    Environment {
        variable: String,
        value: SecretString,
    },
    Stored(Credential),
}

/// A credential refreshed during [`place`] that should be written back.
#[derive(Debug)]
pub struct Refreshed {
    pub key: CredentialKey,
    pub credential: Credential,
}

/// `OPENCLI_<API>_<SCHEME>`, upper-cased with other characters as `_`.
#[must_use]
pub fn environment_variable(api: &ApiName, scheme: &str) -> String {
    let normalize = |raw: &str| -> String {
        raw.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect()
    };
    format!(
        "{ENVIRONMENT_PREFIX}_{}_{}",
        normalize(api.as_str()),
        normalize(scheme)
    )
}

/// Chooses the first security alternative whose schemes all have credentials.
///
/// # Errors
/// [`AuthError::NotLoggedIn`] when no alternative is satisfied and anonymous
/// access isn't one of them; store errors otherwise.
pub fn select(
    api: &ApiName,
    requirements: &[SecurityRequirement],
    schemes: &BTreeMap<String, SecurityScheme>,
    store: &dyn CredentialStore,
    environment: &dyn Fn(&str) -> Option<String>,
) -> Result<Selection, AuthError> {
    if requirements.is_empty() {
        return Ok(Selection::default());
    }
    let mut missing = BTreeSet::new();
    let mut anonymous_allowed = false;
    for requirement in requirements {
        if requirement.schemes.is_empty() {
            anonymous_allowed = true;
            continue;
        }
        let mut items = Vec::new();
        for needed in &requirement.schemes {
            match find(api, &needed.scheme, schemes, store, environment)? {
                Some(item) => items.push(item),
                None => {
                    missing.insert(needed.scheme.clone());
                }
            }
        }
        if items.len() == requirement.schemes.len() {
            return Ok(Selection { items });
        }
    }
    if anonymous_allowed {
        return Ok(Selection::default());
    }
    Err(AuthError::NotLoggedIn {
        api: api.to_string(),
        schemes: missing.into_iter().collect(),
    })
}

fn find(
    api: &ApiName,
    scheme_name: &str,
    schemes: &BTreeMap<String, SecurityScheme>,
    store: &dyn CredentialStore,
    environment: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<Selected>, AuthError> {
    let Some(scheme) = schemes.get(scheme_name) else {
        return Ok(None);
    };
    let key = CredentialKey::new(api, scheme_name);
    let variable = environment_variable(api, scheme_name);
    let source = if let Some(value) = environment(&variable).filter(|value| !value.is_empty()) {
        CredentialSource::Environment {
            variable,
            value: SecretString::from(value),
        }
    } else {
        match store.get(&key)? {
            Some(credential) if fits(scheme, &credential) => CredentialSource::Stored(credential),
            Some(credential) => {
                tracing::warn!(%api, scheme = scheme_name, kind = credential.kind(), "stored credential no longer matches the scheme; ignoring it");
                return Ok(None);
            }
            None => return Ok(None),
        }
    };
    Ok(Some(Selected {
        key,
        scheme_name: scheme_name.to_owned(),
        scheme: scheme.clone(),
        source,
    }))
}

/// A spec refresh can change a scheme's type; old credentials then don't apply.
const fn fits(scheme: &SecurityScheme, credential: &Credential) -> bool {
    matches!(
        (scheme, credential),
        (SecurityScheme::ApiKey { .. }, Credential::ApiKey { .. })
            | (SecurityScheme::HttpBasic, Credential::Basic { .. })
            | (
                SecurityScheme::HttpBearer { .. } | SecurityScheme::OpenIdConnect { .. },
                Credential::Bearer { .. }
            )
            | (SecurityScheme::OAuth2 { .. }, Credential::OAuth2(_))
    )
}

/// Turns selected credentials into placements, refreshing expiring tokens.
///
/// # Errors
/// [`AuthError`] when a token can't be refreshed or a scheme is unsupported.
pub async fn place(
    api: &ApiName,
    selection: Selection,
    tokens: &TokenClient,
    now: OffsetDateTime,
) -> Result<(Vec<CredentialPlacement>, Vec<Refreshed>), AuthError> {
    let mut placements = Vec::new();
    let mut refreshed = Vec::new();
    for item in selection.items {
        match item.source {
            CredentialSource::Environment { variable, value } => {
                placements.push(environment_placement(
                    &item.scheme_name,
                    &item.scheme,
                    &variable,
                    value,
                )?);
            }
            CredentialSource::Stored(Credential::OAuth2(credential)) => {
                let (fresh, changed) = fresh_token(
                    api,
                    &item.scheme_name,
                    &item.scheme,
                    credential,
                    tokens,
                    now,
                )
                .await?;
                placements.push(bearer(fresh.access_token.expose_secret()));
                if changed {
                    refreshed.push(Refreshed {
                        key: item.key,
                        credential: Credential::OAuth2(fresh),
                    });
                }
            }
            CredentialSource::Stored(credential) => {
                placements.push(stored_placement(&item.scheme, &credential));
            }
        }
    }
    Ok((placements, refreshed))
}

fn environment_placement(
    scheme_name: &str,
    scheme: &SecurityScheme,
    variable: &str,
    value: SecretString,
) -> Result<CredentialPlacement, AuthError> {
    match scheme {
        SecurityScheme::ApiKey { name, location } => Ok(api_key(name, *location, value)),
        SecurityScheme::HttpBasic => {
            let (username, password) = value.expose_secret().split_once(':').ok_or_else(|| {
                AuthError::InvalidEnvironmentCredential {
                    variable: variable.to_owned(),
                }
            })?;
            Ok(basic(username, password))
        }
        SecurityScheme::HttpBearer { .. }
        | SecurityScheme::OAuth2 { .. }
        | SecurityScheme::OpenIdConnect { .. } => Ok(bearer(value.expose_secret())),
        SecurityScheme::Unsupported { description } => Err(AuthError::UnsupportedScheme {
            scheme: scheme_name.to_owned(),
            description: description.clone(),
        }),
    }
}

fn stored_placement(scheme: &SecurityScheme, credential: &Credential) -> CredentialPlacement {
    match (scheme, credential) {
        (SecurityScheme::ApiKey { name, location }, Credential::ApiKey { key }) => {
            api_key(name, *location, key.clone())
        }
        (_, Credential::Basic { username, password }) => basic(username, password.expose_secret()),
        (_, Credential::Bearer { token }) => bearer(token.expose_secret()),
        (_, Credential::ApiKey { key }) => bearer(key.expose_secret()),
        (_, Credential::OAuth2(credential)) => bearer(credential.access_token.expose_secret()),
    }
}

async fn fresh_token(
    api: &ApiName,
    scheme_name: &str,
    scheme: &SecurityScheme,
    credential: OAuth2Credential,
    tokens: &TokenClient,
    now: OffsetDateTime,
) -> Result<(OAuth2Credential, bool), AuthError> {
    if credential
        .expires_at
        .is_none_or(|expires_at| expires_at > now + EXPIRY_SKEW)
    {
        return Ok((credential, false));
    }
    let expired = || AuthError::Expired {
        api: api.to_string(),
        scheme: scheme_name.to_owned(),
    };
    let response = match (credential.flow, token_url(scheme, credential.flow)) {
        (GrantFlow::ClientCredentials, Some(url)) => {
            let secret = credential.client_secret.as_ref().ok_or_else(expired)?;
            tokens
                .client_credentials(&url, &credential.client_id, secret, &credential.scopes)
                .await?
        }
        (GrantFlow::AuthorizationCode, Some(url)) => {
            let refresh_token = credential.refresh_token.as_ref().ok_or_else(expired)?;
            tokens
                .refresh(
                    &url,
                    &credential.client_id,
                    credential.client_secret.as_ref(),
                    refresh_token,
                )
                .await?
        }
        (_, None) => return Err(expired()),
    };
    let expires_at = response.expires_at(now);
    Ok((
        OAuth2Credential {
            access_token: response.access_token,
            // Providers that rotate refresh tokens send a new one; others keep the old.
            refresh_token: response.refresh_token.or(credential.refresh_token),
            expires_at,
            ..credential
        },
        true,
    ))
}

/// The spec is authoritative for endpoints, so a changed token URL takes effect.
fn token_url(scheme: &SecurityScheme, flow: GrantFlow) -> Option<String> {
    let SecurityScheme::OAuth2 { flows } = scheme else {
        return None;
    };
    flows.iter().find_map(|declared| match (declared, flow) {
        (OAuth2Flow::ClientCredentials { token_url, .. }, GrantFlow::ClientCredentials) => {
            Some(token_url.clone())
        }
        (
            OAuth2Flow::AuthorizationCode {
                token_url,
                refresh_url,
                ..
            },
            GrantFlow::AuthorizationCode,
        ) => Some(refresh_url.clone().unwrap_or_else(|| token_url.clone())),
        _ => None,
    })
}

fn api_key(name: &str, location: ApiKeyLocation, value: SecretString) -> CredentialPlacement {
    let location = match location {
        ApiKeyLocation::Header => CredentialLocation::Header,
        ApiKeyLocation::Query => CredentialLocation::Query,
        ApiKeyLocation::Cookie => CredentialLocation::Cookie,
    };
    CredentialPlacement {
        location,
        name: name.to_owned(),
        value,
    }
}

fn basic(username: &str, password: &str) -> CredentialPlacement {
    let encoded = BASE64.encode(format!("{username}:{password}"));
    header(AUTHORIZATION, format!("Basic {encoded}"))
}

fn bearer(token: &str) -> CredentialPlacement {
    header(AUTHORIZATION, format!("Bearer {token}"))
}

fn header(name: &str, value: String) -> CredentialPlacement {
    CredentialPlacement {
        location: CredentialLocation::Header,
        name: name.to_owned(),
        value: SecretString::from(value),
    }
}
