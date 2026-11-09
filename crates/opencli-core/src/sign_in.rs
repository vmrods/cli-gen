//! Sign-in operations whose returned token becomes the API's credential
//! (ADR 0010): which commands sign in, which scheme the token is for, and
//! where the token sits in the response.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::{ApiKeyLocation, CommandSpec, HttpMethod, SecurityScheme};

/// Normalized last path words that mark a sign-in, best first.
const EXACT_WORDS: &[&str] = &["login", "signin", "authenticate", "token"];
/// Endings that also mark one (`federated-login`, `refresh-token`).
const WORD_ENDINGS: &[&str] = &["login", "signin", "token"];
/// Response fields that hold the token, checked at the top level and then
/// inside the usual envelopes.
const TOKEN_FIELDS: &[&str] = &["token", "accessToken", "access_token", "jwt"];
const ENVELOPES: &[&str] = &["data", "result"];
const AUTHORIZATION_HEADER: &str = "authorization";

/// How a saved token is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenForm {
    /// An API key in the `Authorization` header: saved as `Bearer <token>`,
    /// because the key is sent exactly as stored.
    AuthorizationHeader,
    /// An HTTP bearer scheme: saved bare; the runtime adds `Bearer `.
    BearerToken,
}

/// Where a sign-in command's token goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignIn {
    pub scheme: String,
    pub form: TokenForm,
    /// Lower is a better match (`login` beats `federated-login`).
    pub rank: u8,
}

/// Whether `command` signs in: a POST whose last path word is a sign-in word,
/// on an API with exactly one scheme a returned token can fill.
#[must_use]
pub fn sign_in_target(
    command: &CommandSpec,
    schemes: &BTreeMap<String, SecurityScheme>,
) -> Option<SignIn> {
    if command.method != HttpMethod::Post {
        return None;
    }
    let rank = sign_in_rank(&command.path_template)?;
    token_scheme(schemes).map(|(scheme, form)| SignIn { scheme, form, rank })
}

/// The API's one scheme a token can fill (an `Authorization` API key or an
/// HTTP bearer scheme), or `None` when it has none or several: guessing could
/// put a token where it doesn't belong. Shared by sign-ins (ADR 0010) and
/// linked credentials (ADR 0011).
#[must_use]
pub fn token_scheme(schemes: &BTreeMap<String, SecurityScheme>) -> Option<(String, TokenForm)> {
    let mut targets = schemes
        .iter()
        .filter_map(|(name, scheme)| token_form(scheme).map(|form| (name.clone(), form)));
    let target = targets.next()?;
    targets.next().is_none().then_some(target)
}

fn sign_in_rank(path: &str) -> Option<u8> {
    let last = path
        .split('/')
        .rev()
        .find(|segment| !segment.is_empty() && !segment.contains('{'))?;
    let word: String = last
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect();
    if EXACT_WORDS.contains(&word.as_str()) {
        Some(0)
    } else if WORD_ENDINGS.iter().any(|ending| word.ends_with(ending)) {
        Some(1)
    } else {
        None
    }
}

fn token_form(scheme: &SecurityScheme) -> Option<TokenForm> {
    match scheme {
        SecurityScheme::ApiKey {
            name,
            location: ApiKeyLocation::Header,
        } if name.eq_ignore_ascii_case(AUTHORIZATION_HEADER) => {
            Some(TokenForm::AuthorizationHeader)
        }
        SecurityScheme::HttpBearer { .. } => Some(TokenForm::BearerToken),
        _ => None,
    }
}

/// The token in a sign-in response: a non-empty string field named like a
/// token, at the top level or under `data`/`result`.
#[must_use]
pub fn find_token(response: &Value) -> Option<&str> {
    std::iter::once(response)
        .chain(
            ENVELOPES
                .iter()
                .filter_map(|envelope| response.get(envelope)),
        )
        .flat_map(|object| TOKEN_FIELDS.iter().filter_map(|field| object.get(field)))
        .filter_map(Value::as_str)
        .find(|token| !token.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::json;

    use super::*;
    use crate::{ArgumentSpec, CommandSpec};

    fn command(method: HttpMethod, path: &str) -> CommandSpec {
        CommandSpec {
            path: Vec::new(),
            method,
            path_template: path.to_owned(),
            operation_id: None,
            summary: None,
            description: None,
            deprecated: false,
            arguments: Vec::<ArgumentSpec>::new(),
            body: None,
            security: Vec::new(),
        }
    }

    fn authorization_key() -> SecurityScheme {
        SecurityScheme::ApiKey {
            name: "Authorization".to_owned(),
            location: ApiKeyLocation::Header,
        }
    }

    #[rstest]
    #[case(HttpMethod::Post, "/api/v1/account/login", Some(0))]
    #[case(HttpMethod::Post, "/auth/sign-in", Some(0))]
    #[case(HttpMethod::Post, "/oauth/token", Some(0))]
    #[case(HttpMethod::Post, "/account/federated-login", Some(1))]
    #[case(HttpMethod::Post, "/account/refresh-token", Some(1))]
    #[case(HttpMethod::Post, "/sessions/{id}/login", Some(0))]
    #[case(HttpMethod::Get, "/account/login", None)]
    #[case(HttpMethod::Post, "/account/logout", None)]
    #[case(HttpMethod::Post, "/account/is-token-valid", None)]
    fn sign_in_target_recognizes_sign_in_paths(
        #[case] method: HttpMethod,
        #[case] path: &str,
        #[case] rank: Option<u8>,
    ) {
        let schemes = BTreeMap::from([("Bearer".to_owned(), authorization_key())]);
        assert_eq!(
            sign_in_target(&command(method, path), &schemes).map(|target| target.rank),
            rank
        );
    }

    #[test]
    fn sign_in_target_picks_the_one_scheme_a_token_can_fill() {
        let schemes = BTreeMap::from([
            ("Bearer".to_owned(), authorization_key()),
            ("basic".to_owned(), SecurityScheme::HttpBasic),
        ]);
        assert_eq!(
            sign_in_target(&command(HttpMethod::Post, "/login"), &schemes),
            Some(SignIn {
                scheme: "Bearer".to_owned(),
                form: TokenForm::AuthorizationHeader,
                rank: 0,
            })
        );
        let bearer = BTreeMap::from([(
            "jwt".to_owned(),
            SecurityScheme::HttpBearer {
                bearer_format: None,
            },
        )]);
        assert_eq!(
            sign_in_target(&command(HttpMethod::Post, "/login"), &bearer).map(|t| t.form),
            Some(TokenForm::BearerToken)
        );
    }

    #[test]
    fn sign_in_target_refuses_to_guess_between_schemes() {
        let schemes = BTreeMap::from([
            ("a".to_owned(), authorization_key()),
            (
                "b".to_owned(),
                SecurityScheme::HttpBearer {
                    bearer_format: None,
                },
            ),
        ]);
        assert_eq!(
            sign_in_target(&command(HttpMethod::Post, "/login"), &schemes),
            None
        );
        let other_header = BTreeMap::from([(
            "key".to_owned(),
            SecurityScheme::ApiKey {
                name: "X-Api-Key".to_owned(),
                location: ApiKeyLocation::Header,
            },
        )]);
        assert_eq!(
            sign_in_target(&command(HttpMethod::Post, "/login"), &other_header),
            None
        );
    }

    #[rstest]
    #[case(json!({"token": "abc"}), Some("abc"))]
    #[case(json!({"data": {"token": "abc", "refreshToken": "r"}}), Some("abc"))]
    #[case(json!({"access_token": "abc", "token_type": "bearer"}), Some("abc"))]
    #[case(json!({"result": {"accessToken": "abc"}}), Some("abc"))]
    #[case(json!({"data": {"token": ""}}), None)]
    #[case(json!({"data": {"token": null}}), None)]
    #[case(json!({"id": 1}), None)]
    #[case(json!([{"token": "abc"}]), None)]
    fn find_token_reads_common_shapes(#[case] response: Value, #[case] expected: Option<&str>) {
        assert_eq!(find_token(&response), expected);
    }
}
