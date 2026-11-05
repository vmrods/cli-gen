use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// A specification normalized from any supported format (ADR 0001).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiSpec {
    pub format: SpecFormat,
    pub title: String,
    pub version: String,
    pub description: Option<String>,
    pub servers: Vec<String>,
    pub operations: Vec<Operation>,
    pub security_schemes: BTreeMap<String, SecurityScheme>,
    /// Named schemas (`components.schemas`), kept as JSON Schema with `$ref`s intact.
    pub schemas: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpecFormat {
    #[serde(rename = "openapi-3.0")]
    OpenApi30,
}

impl SpecFormat {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenApi30 => "openapi-3.0",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Operation {
    pub operation_id: Option<String>,
    pub method: HttpMethod,
    pub path: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub deprecated: bool,
    pub parameters: Vec<Parameter>,
    pub request_body: Option<RequestBody>,
    /// Effective requirements after inheriting the document default. Any one
    /// alternative satisfies the operation; empty means no authentication.
    pub security: Vec<SecurityRequirement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub location: ParameterLocation,
    pub required: bool,
    pub description: Option<String>,
    pub schema: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParameterLocation {
    Path,
    Query,
    Header,
    Cookie,
}

impl ParameterLocation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Query => "query",
            Self::Header => "header",
            Self::Cookie => "cookie",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestBody {
    pub required: bool,
    pub content_type: String,
    pub description: Option<String>,
    pub schema: Option<Value>,
}

/// Schemes that must all be satisfied together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityRequirement {
    pub schemes: Vec<SchemeRequirement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemeRequirement {
    pub scheme: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SecurityScheme {
    ApiKey {
        name: String,
        location: ApiKeyLocation,
    },
    HttpBasic,
    HttpBearer {
        bearer_format: Option<String>,
    },
    #[serde(rename = "oauth2")]
    OAuth2 {
        flows: Vec<OAuth2Flow>,
    },
    OpenIdConnect {
        url: String,
    },
    /// Declared by the spec but not supported by OpenCLI; operations that
    /// require it fail at call time with this description.
    Unsupported {
        description: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyLocation {
    Header,
    Query,
    Cookie,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "flow", rename_all = "kebab-case")]
pub enum OAuth2Flow {
    ClientCredentials {
        token_url: String,
        refresh_url: Option<String>,
        scopes: BTreeMap<String, String>,
    },
    AuthorizationCode {
        authorization_url: String,
        token_url: String,
        refresh_url: Option<String>,
        scopes: BTreeMap<String, String>,
    },
}

/// A stored string that doesn't name a known enum variant.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("unknown {kind} `{value}`")]
pub struct UnknownVariant {
    pub kind: &'static str,
    pub value: String,
}

macro_rules! string_enum {
    ($type:ty, $kind:literal, [$($variant:expr),+ $(,)?]) => {
        impl fmt::Display for $type {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $type {
            type Err = UnknownVariant;

            fn from_str(raw: &str) -> Result<Self, Self::Err> {
                [$($variant),+]
                    .into_iter()
                    .find(|variant| variant.as_str() == raw)
                    .ok_or_else(|| UnknownVariant { kind: $kind, value: raw.to_owned() })
            }
        }
    };
}

string_enum!(SpecFormat, "spec format", [SpecFormat::OpenApi30]);
string_enum!(
    HttpMethod,
    "HTTP method",
    [
        HttpMethod::Get,
        HttpMethod::Post,
        HttpMethod::Put,
        HttpMethod::Patch,
        HttpMethod::Delete,
    ]
);
string_enum!(
    ParameterLocation,
    "parameter location",
    [
        ParameterLocation::Path,
        ParameterLocation::Query,
        ParameterLocation::Header,
        ParameterLocation::Cookie,
    ]
);

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(HttpMethod::Get)]
    #[case(HttpMethod::Post)]
    #[case(HttpMethod::Put)]
    #[case(HttpMethod::Patch)]
    #[case(HttpMethod::Delete)]
    fn http_method_round_trips_through_string(#[case] method: HttpMethod) {
        assert_eq!(method.to_string().parse::<HttpMethod>(), Ok(method));
    }

    #[rstest]
    #[case(ParameterLocation::Path)]
    #[case(ParameterLocation::Query)]
    #[case(ParameterLocation::Header)]
    #[case(ParameterLocation::Cookie)]
    fn parameter_location_round_trips_through_string(#[case] location: ParameterLocation) {
        assert_eq!(
            location.to_string().parse::<ParameterLocation>(),
            Ok(location)
        );
    }

    #[test]
    fn spec_format_round_trips_through_string() {
        assert_eq!("openapi-3.0".parse(), Ok(SpecFormat::OpenApi30));
    }

    #[test]
    fn from_str_rejects_unknown_value() {
        assert_eq!(
            "TRACE".parse::<HttpMethod>(),
            Err(UnknownVariant {
                kind: "HTTP method",
                value: "TRACE".into()
            })
        );
    }

    #[test]
    fn security_scheme_serializes_with_type_tag() {
        let scheme = SecurityScheme::ApiKey {
            name: "X-Key".into(),
            location: ApiKeyLocation::Header,
        };
        let json = serde_json::to_value(&scheme).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"type": "api-key", "name": "X-Key", "location": "header"})
        );
        assert_eq!(
            serde_json::from_value::<SecurityScheme>(json).unwrap(),
            scheme
        );
    }
}
