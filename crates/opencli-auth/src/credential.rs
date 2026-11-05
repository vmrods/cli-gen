//! What is stored per (API, security scheme). Secret fields are
//! [`SecretString`]s, so they never appear in `Debug` output or logs.

use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Credential {
    ApiKey {
        #[serde(with = "secret")]
        key: SecretString,
    },
    Bearer {
        #[serde(with = "secret")]
        token: SecretString,
    },
    Basic {
        username: String,
        #[serde(with = "secret")]
        password: SecretString,
    },
    #[serde(rename = "oauth2")]
    OAuth2(OAuth2Credential),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2Credential {
    pub flow: GrantFlow,
    pub client_id: String,
    #[serde(with = "optional_secret", default)]
    pub client_secret: Option<SecretString>,
    pub scopes: Vec<String>,
    #[serde(with = "secret")]
    pub access_token: SecretString,
    #[serde(with = "optional_secret", default)]
    pub refresh_token: Option<SecretString>,
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub expires_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GrantFlow {
    ClientCredentials,
    AuthorizationCode,
}

impl Credential {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::ApiKey { .. } => "api-key",
            Self::Bearer { .. } => "bearer",
            Self::Basic { .. } => "basic",
            Self::OAuth2(_) => "oauth2",
        }
    }
}

/// Secrets are written in clear only into an encrypted or OS-protected store.
mod secret {
    use secrecy::{ExposeSecret, SecretString};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        value: &SecretString,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(value.expose_secret())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<SecretString, D::Error> {
        String::deserialize(deserializer).map(SecretString::from)
    }
}

mod optional_secret {
    use secrecy::{ExposeSecret, SecretString};
    use serde::{Deserialize, Deserializer, Serializer};

    // serde's `with` passes `&Option<T>`.
    #[allow(clippy::ref_option)]
    pub fn serialize<S: Serializer>(
        value: &Option<SecretString>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(secret) => serializer.serialize_some(secret.expose_secret()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<SecretString>, D::Error> {
        Option::<String>::deserialize(deserializer).map(|value| value.map(SecretString::from))
    }
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;
    use time::macros::datetime;

    use super::*;

    #[test]
    fn credentials_round_trip_through_json() {
        let credential = Credential::OAuth2(OAuth2Credential {
            flow: GrantFlow::ClientCredentials,
            client_id: "app".into(),
            client_secret: Some(SecretString::from("s3cret")),
            scopes: vec!["read".into()],
            access_token: SecretString::from("t0ken"),
            refresh_token: None,
            expires_at: Some(datetime!(2026-11-05 12:00 UTC)),
        });
        let json = serde_json::to_string(&credential).unwrap();
        let Credential::OAuth2(decoded) = serde_json::from_str(&json).unwrap() else {
            panic!("expected OAuth2 credential");
        };
        assert_eq!(decoded.access_token.expose_secret(), "t0ken");
        assert_eq!(decoded.client_secret.unwrap().expose_secret(), "s3cret");
        assert_eq!(decoded.expires_at, Some(datetime!(2026-11-05 12:00 UTC)));
    }

    #[test]
    fn debug_output_never_contains_secrets() {
        let credential = Credential::Basic {
            username: "me".into(),
            password: SecretString::from("hunter2"),
        };
        assert!(!format!("{credential:?}").contains("hunter2"));
    }
}
