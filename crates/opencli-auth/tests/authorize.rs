// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::Duration;

use opencli_auth::{
    AuthError, Credential, CredentialKey, CredentialStore, CredentialStoreError, GrantFlow,
    OAuth2Credential, TokenClient, environment_variable, place, select,
};
use opencli_core::{
    ApiKeyLocation, ApiName, OAuth2Flow, SchemeRequirement, SecurityRequirement, SecurityScheme,
};
use opencli_runtime::{CredentialLocation, CredentialPlacement};
use secrecy::{ExposeSecret, SecretString};
use time::OffsetDateTime;
use time::macros::datetime;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const NOW: OffsetDateTime = datetime!(2026-11-05 12:00 UTC);

/// In-memory stand-in for the keyring.
#[derive(Default)]
struct FakeStore(Mutex<HashMap<String, Credential>>);

impl FakeStore {
    fn with(entries: &[(&str, Credential)]) -> Self {
        let store = Self::default();
        for (scheme, credential) in entries {
            store.set(&key(scheme), credential).unwrap();
        }
        store
    }
}

impl CredentialStore for FakeStore {
    fn name(&self) -> &'static str {
        "fake"
    }
    fn get(&self, key: &CredentialKey) -> Result<Option<Credential>, CredentialStoreError> {
        Ok(self.0.lock().unwrap().get(&key.account()).cloned())
    }
    fn set(
        &self,
        key: &CredentialKey,
        credential: &Credential,
    ) -> Result<(), CredentialStoreError> {
        self.0
            .lock()
            .unwrap()
            .insert(key.account(), credential.clone());
        Ok(())
    }
    fn delete(&self, key: &CredentialKey) -> Result<bool, CredentialStoreError> {
        Ok(self.0.lock().unwrap().remove(&key.account()).is_some())
    }
}

fn api() -> ApiName {
    ApiName::parse("crm").unwrap()
}

fn key(scheme: &str) -> CredentialKey {
    CredentialKey::new(&api(), scheme)
}

fn secret(value: &str) -> SecretString {
    SecretString::from(value.to_owned())
}

fn requirement(schemes: &[&str]) -> SecurityRequirement {
    SecurityRequirement {
        schemes: schemes
            .iter()
            .map(|&scheme| SchemeRequirement {
                scheme: scheme.into(),
                scopes: vec![],
            })
            .collect(),
    }
}

fn schemes(token_url: &str) -> BTreeMap<String, SecurityScheme> {
    BTreeMap::from([
        (
            "header".into(),
            SecurityScheme::ApiKey {
                name: "X-API-Key".into(),
                location: ApiKeyLocation::Header,
            },
        ),
        (
            "query".into(),
            SecurityScheme::ApiKey {
                name: "api_key".into(),
                location: ApiKeyLocation::Query,
            },
        ),
        ("basic".into(), SecurityScheme::HttpBasic),
        (
            "bearer".into(),
            SecurityScheme::HttpBearer {
                bearer_format: None,
            },
        ),
        (
            "legacy".into(),
            SecurityScheme::Unsupported {
                description: "HTTP digest".into(),
            },
        ),
        (
            "oauth".into(),
            SecurityScheme::OAuth2 {
                flows: vec![
                    OAuth2Flow::ClientCredentials {
                        token_url: token_url.into(),
                        refresh_url: None,
                        scopes: BTreeMap::new(),
                    },
                    OAuth2Flow::AuthorizationCode {
                        authorization_url: "https://login.example.com/authorize".into(),
                        token_url: token_url.into(),
                        refresh_url: None,
                        scopes: BTreeMap::new(),
                    },
                ],
            },
        ),
    ])
}

fn no_environment(_: &str) -> Option<String> {
    None
}

fn tokens() -> TokenClient {
    TokenClient::new(Duration::from_secs(5)).unwrap()
}

async fn placements(
    requirements: &[SecurityRequirement],
    store: &FakeStore,
    environment: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<(CredentialLocation, String, String)>, AuthError> {
    let selection = select(
        &api(),
        requirements,
        &schemes("https://auth.example.com/token"),
        store,
        environment,
    )?;
    let (placements, _) = place(&api(), selection, &tokens(), NOW).await?;
    Ok(placements.into_iter().map(expose).collect())
}

fn expose(placement: CredentialPlacement) -> (CredentialLocation, String, String) {
    (
        placement.location,
        placement.name,
        placement.value.expose_secret().to_owned(),
    )
}

fn oauth(flow: GrantFlow, expires_at: OffsetDateTime, refresh_token: Option<&str>) -> Credential {
    Credential::OAuth2(OAuth2Credential {
        flow,
        client_id: "app".into(),
        client_secret: Some(secret("s3cret")),
        scopes: vec!["read".into()],
        access_token: secret("old-token"),
        refresh_token: refresh_token.map(secret),
        expires_at: Some(expires_at),
    })
}

#[test]
fn environment_variable_is_upper_snake_case() {
    assert_eq!(
        environment_variable(&ApiName::parse("crm-api").unwrap(), "apiKey"),
        "OPENCLI_CRM_API_APIKEY"
    );
    assert_eq!(
        environment_variable(&api(), "oauth2.client"),
        "OPENCLI_CRM_OAUTH2_CLIENT"
    );
}

#[tokio::test]
async fn operations_without_security_send_nothing() {
    assert_eq!(
        placements(&[], &FakeStore::default(), &no_environment)
            .await
            .unwrap(),
        []
    );
}

#[tokio::test]
async fn stored_credentials_are_placed_per_scheme_type() {
    let store = FakeStore::with(&[
        ("header", Credential::ApiKey { key: secret("k-1") }),
        ("query", Credential::ApiKey { key: secret("k-2") }),
        (
            "basic",
            Credential::Basic {
                username: "me".into(),
                password: secret("pw"),
            },
        ),
        ("bearer", Credential::Bearer { token: secret("t") }),
    ]);
    let cases = [
        ("header", (CredentialLocation::Header, "X-API-Key", "k-1")),
        ("query", (CredentialLocation::Query, "api_key", "k-2")),
        (
            "basic",
            (
                CredentialLocation::Header,
                "Authorization",
                "Basic bWU6cHc=",
            ),
        ),
        (
            "bearer",
            (CredentialLocation::Header, "Authorization", "Bearer t"),
        ),
    ];
    for (scheme, (location, name, value)) in cases {
        let found = placements(&[requirement(&[scheme])], &store, &no_environment)
            .await
            .unwrap();
        assert_eq!(
            found,
            [(location, name.to_owned(), value.to_owned())],
            "{scheme}"
        );
    }
}

#[tokio::test]
async fn environment_overrides_stored_credentials() {
    let store = FakeStore::with(&[(
        "bearer",
        Credential::Bearer {
            token: secret("stored"),
        },
    )]);
    let environment = |name: &str| (name == "OPENCLI_CRM_BEARER").then(|| "from-env".to_owned());
    let found = placements(&[requirement(&["bearer"])], &store, &environment)
        .await
        .unwrap();
    assert_eq!(found[0].2, "Bearer from-env");
}

#[tokio::test]
async fn environment_basic_credentials_need_a_colon() {
    let environment = |_: &str| Some("no-colon".to_owned());
    let error = placements(
        &[requirement(&["basic"])],
        &FakeStore::default(),
        &environment,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&error, AuthError::InvalidEnvironmentCredential { variable } if variable == "OPENCLI_CRM_BASIC")
    );
}

#[tokio::test]
async fn first_satisfied_alternative_wins() {
    let store = FakeStore::with(&[("bearer", Credential::Bearer { token: secret("t") })]);
    let found = placements(
        &[requirement(&["header"]), requirement(&["bearer"])],
        &store,
        &no_environment,
    )
    .await
    .unwrap();
    assert_eq!(found[0].2, "Bearer t");
}

#[tokio::test]
async fn combined_requirements_need_every_scheme() {
    let store = FakeStore::with(&[("header", Credential::ApiKey { key: secret("k") })]);
    let error = placements(
        &[requirement(&["header", "bearer"])],
        &store,
        &no_environment,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&error, AuthError::NotLoggedIn { schemes, .. } if schemes == &["bearer"]),
        "{error:?}"
    );

    let store = FakeStore::with(&[
        ("header", Credential::ApiKey { key: secret("k") }),
        ("bearer", Credential::Bearer { token: secret("t") }),
    ]);
    assert_eq!(
        placements(
            &[requirement(&["header", "bearer"])],
            &store,
            &no_environment
        )
        .await
        .unwrap()
        .len(),
        2
    );
}

#[tokio::test]
async fn optional_security_falls_back_to_anonymous() {
    let found = placements(
        &[requirement(&["bearer"]), requirement(&[])],
        &FakeStore::default(),
        &no_environment,
    )
    .await
    .unwrap();
    assert_eq!(found, []);
}

#[tokio::test]
async fn missing_credentials_list_every_option() {
    let error = placements(
        &[requirement(&["header"]), requirement(&["bearer"])],
        &FakeStore::default(),
        &no_environment,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&error, AuthError::NotLoggedIn { api, schemes } if api == "crm" && schemes == &["bearer", "header"])
    );
}

#[tokio::test]
async fn credentials_of_the_wrong_type_are_ignored() {
    let store = FakeStore::with(&[("bearer", Credential::ApiKey { key: secret("k") })]);
    let error = placements(&[requirement(&["bearer"])], &store, &no_environment)
        .await
        .unwrap_err();
    assert!(matches!(error, AuthError::NotLoggedIn { .. }));
}

#[tokio::test]
async fn unsupported_schemes_are_reported() {
    let environment = |_: &str| Some("x".to_owned());
    let error = placements(
        &[requirement(&["legacy"])],
        &FakeStore::default(),
        &environment,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, AuthError::UnsupportedScheme { .. }));
}

#[tokio::test]
async fn valid_oauth_tokens_are_used_without_refresh() {
    let store = FakeStore::with(&[(
        "oauth",
        oauth(
            GrantFlow::ClientCredentials,
            NOW + time::Duration::hours(1),
            None,
        ),
    )]);
    let selection = select(
        &api(),
        &[requirement(&["oauth"])],
        &schemes("https://unused.example.com"),
        &store,
        &no_environment,
    )
    .unwrap();
    let (found, refreshed) = place(&api(), selection, &tokens(), NOW).await.unwrap();
    assert_eq!(
        expose(found.into_iter().next().unwrap()).2,
        "Bearer old-token"
    );
    assert!(refreshed.is_empty());
}

#[tokio::test]
async fn expiring_client_credentials_tokens_are_renewed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=client_credentials"))
        .and(body_string_contains("client_secret=s3cret"))
        .and(body_string_contains("scope=read"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "new-token", "token_type": "Bearer", "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;
    // Within the 60-second skew, so treated as expired.
    let store = FakeStore::with(&[(
        "oauth",
        oauth(
            GrantFlow::ClientCredentials,
            NOW + time::Duration::seconds(30),
            None,
        ),
    )]);
    let selection = select(
        &api(),
        &[requirement(&["oauth"])],
        &schemes(&format!("{}/token", server.uri())),
        &store,
        &no_environment,
    )
    .unwrap();

    let (found, refreshed) = place(&api(), selection, &tokens(), NOW).await.unwrap();

    assert_eq!(
        expose(found.into_iter().next().unwrap()).2,
        "Bearer new-token"
    );
    let Credential::OAuth2(renewed) = &refreshed[0].credential else {
        panic!("expected OAuth2");
    };
    assert_eq!(refreshed[0].key, key("oauth"));
    assert_eq!(renewed.expires_at, Some(NOW + time::Duration::hours(1)));
}

#[tokio::test]
async fn expired_authorization_code_tokens_use_the_refresh_token() {
    let server = MockServer::start().await;
    Mock::given(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=r-1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"access_token": "new-token"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let store = FakeStore::with(&[(
        "oauth",
        oauth(
            GrantFlow::AuthorizationCode,
            NOW - time::Duration::hours(1),
            Some("r-1"),
        ),
    )]);
    let selection = select(
        &api(),
        &[requirement(&["oauth"])],
        &schemes(&format!("{}/token", server.uri())),
        &store,
        &no_environment,
    )
    .unwrap();
    let (_, refreshed) = place(&api(), selection, &tokens(), NOW).await.unwrap();
    let Credential::OAuth2(renewed) = &refreshed[0].credential else {
        panic!("expected OAuth2");
    };
    assert_eq!(renewed.access_token.expose_secret(), "new-token");
    assert_eq!(
        renewed.refresh_token.as_ref().unwrap().expose_secret(),
        "r-1",
        "kept when not rotated"
    );
    assert_eq!(renewed.expires_at, None);
}

#[tokio::test]
async fn expired_tokens_without_refresh_token_need_login() {
    let store = FakeStore::with(&[(
        "oauth",
        oauth(
            GrantFlow::AuthorizationCode,
            NOW - time::Duration::hours(1),
            None,
        ),
    )]);
    let selection = select(
        &api(),
        &[requirement(&["oauth"])],
        &schemes("https://auth.example.com/token"),
        &store,
        &no_environment,
    )
    .unwrap();
    let error = place(&api(), selection, &tokens(), NOW).await.unwrap_err();
    assert!(matches!(error, AuthError::Expired { .. }), "{error:?}");
}
