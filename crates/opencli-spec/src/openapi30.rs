//! OpenAPI 3.0 adapter: `openapiv3` types → domain model. 3.1 and Swagger 2.0
//! documents arrive here already rewritten to 3.0 form.

use std::collections::BTreeMap;

use openapiv3 as oa;
use opencli_core::{
    ApiKeyLocation, ApiSpec, CliHints, HttpMethod, OAuth2Flow, Operation, Parameter,
    ParameterLocation, RequestBody, SchemeRequirement, SecurityRequirement, SecurityScheme,
    SpecFormat,
};
use serde::Serialize;
use serde_json::Value;

use crate::{SpecError, ValidationIssue};

const PARAMETERS_PREFIX: &str = "#/components/parameters/";
const REQUEST_BODIES_PREFIX: &str = "#/components/requestBodies/";
const SECURITY_SCHEMES_PREFIX: &str = "#/components/securitySchemes/";
const MAX_REFERENCE_DEPTH: usize = 16;
const JSON_MEDIA_TYPE: &str = "application/json";
const CLI_NAME_EXTENSION: &str = "x-cli-name";
const CLI_RESOURCE_EXTENSION: &str = "x-cli-resource";

pub(crate) fn convert(
    document: Value,
    format: SpecFormat,
) -> Result<(ApiSpec, Vec<ValidationIssue>), SpecError> {
    let openapi: oa::OpenAPI = serde_path_to_error::deserialize(document).map_err(|error| {
        let path = error.path().to_string();
        SpecError::Structure {
            // serde_path_to_error renders the root as ".".
            path: if path == "." {
                "document root".to_owned()
            } else {
                path
            },
            message: error.inner().to_string(),
        }
    })?;
    let no_components = oa::Components::default();
    let mut converter = Converter {
        components: openapi.components.as_ref().unwrap_or(&no_components),
        default_security: openapi.security.as_deref().unwrap_or_default(),
        issues: Vec::new(),
    };

    let operations = converter.operations(&openapi.paths);
    let security_schemes = converter.security_schemes();
    let schemas = converter.schemas();
    let spec = ApiSpec {
        format,
        title: openapi.info.title.clone(),
        version: openapi.info.version.clone(),
        description: openapi.info.description.clone(),
        servers: openapi.servers.iter().map(server_url).collect(),
        operations,
        security_schemes,
        schemas,
    };
    Ok((spec, converter.issues))
}

struct Converter<'a> {
    components: &'a oa::Components,
    default_security: &'a [oa::SecurityRequirement],
    issues: Vec<ValidationIssue>,
}

impl<'a> Converter<'a> {
    fn operations(&mut self, paths: &'a oa::Paths) -> Vec<Operation> {
        let mut operations = Vec::new();
        for (path, item) in &paths.paths {
            let oa::ReferenceOr::Item(item) = item else {
                self.issues.push(ValidationIssue::new(
                    path.as_str(),
                    "path item references are not supported; inline the path item",
                ));
                continue;
            };
            let methods = [
                (HttpMethod::Get, &item.get),
                (HttpMethod::Post, &item.post),
                (HttpMethod::Put, &item.put),
                (HttpMethod::Patch, &item.patch),
                (HttpMethod::Delete, &item.delete),
            ];
            for (method, operation) in methods {
                if let Some(operation) = operation {
                    operations.push(self.operation(method, path, &item.parameters, operation));
                }
            }
        }
        operations
    }

    fn operation(
        &mut self,
        method: HttpMethod,
        path: &str,
        path_parameters: &'a [oa::ReferenceOr<oa::Parameter>],
        operation: &'a oa::Operation,
    ) -> Operation {
        let location = format!("{method} {path}");
        let mut parameters: Vec<Parameter> = Vec::new();
        // Operation-level parameters override path-level ones with the same name and location.
        for reference in path_parameters.iter().chain(&operation.parameters) {
            let Some(parameter) = self.parameter(reference, &location) else {
                continue;
            };
            parameters.retain(|existing| {
                (existing.name.as_str(), existing.location)
                    != (parameter.name.as_str(), parameter.location)
            });
            parameters.push(parameter);
        }
        let request_body = operation
            .request_body
            .as_ref()
            .and_then(|body| self.request_body(body, &location));
        let security = operation
            .security
            .as_deref()
            .unwrap_or(self.default_security)
            .iter()
            .map(security_requirement)
            .collect();

        Operation {
            operation_id: operation.operation_id.clone(),
            method,
            path: path.to_owned(),
            summary: operation.summary.clone(),
            description: operation.description.clone(),
            deprecated: operation.deprecated,
            parameters,
            request_body,
            security,
            cli: self.cli_hints(operation, &location),
        }
    }

    fn cli_hints(&mut self, operation: &oa::Operation, location: &str) -> CliHints {
        let name = match operation.extensions.get(CLI_NAME_EXTENSION) {
            None => None,
            Some(Value::String(name)) if !name.trim().is_empty() => Some(name.trim().to_owned()),
            Some(_) => {
                self.issues.push(ValidationIssue::new(
                    location,
                    format!("`{CLI_NAME_EXTENSION}` must be a non-empty string"),
                ));
                None
            }
        };
        let resource = match operation.extensions.get(CLI_RESOURCE_EXTENSION) {
            None => None,
            Some(value) => {
                let words = resource_words(value);
                if words.is_none() {
                    self.issues.push(ValidationIssue::new(
                        location,
                        format!("`{CLI_RESOURCE_EXTENSION}` must be a string of words or a non-empty array of strings"),
                    ));
                }
                words
            }
        };
        CliHints { name, resource }
    }

    fn parameter(
        &mut self,
        reference: &'a oa::ReferenceOr<oa::Parameter>,
        location: &str,
    ) -> Option<Parameter> {
        let components = self.components;
        let parameter = self.resolve(reference, PARAMETERS_PREFIX, location, |name| {
            components.parameters.get(name)
        })?;
        let (data, parameter_location) = match parameter {
            oa::Parameter::Query { parameter_data, .. } => {
                (parameter_data, ParameterLocation::Query)
            }
            oa::Parameter::Header { parameter_data, .. } => {
                (parameter_data, ParameterLocation::Header)
            }
            oa::Parameter::Path { parameter_data, .. } => (parameter_data, ParameterLocation::Path),
            oa::Parameter::Cookie { parameter_data, .. } => {
                (parameter_data, ParameterLocation::Cookie)
            }
        };
        let schema = match &data.format {
            oa::ParameterSchemaOrContent::Schema(schema) => self.normalize(schema, location),
            oa::ParameterSchemaOrContent::Content(content) => content
                .values()
                .next()
                .and_then(|media| media.schema.as_ref())
                .and_then(|schema| self.normalize(schema, location)),
        };
        Some(Parameter {
            name: data.name.clone(),
            location: parameter_location,
            required: data.required,
            description: data.description.clone(),
            schema,
            array_delimiter: array_delimiter(parameter),
        })
    }

    fn request_body(
        &mut self,
        reference: &'a oa::ReferenceOr<oa::RequestBody>,
        location: &str,
    ) -> Option<RequestBody> {
        let components = self.components;
        let body = self.resolve(reference, REQUEST_BODIES_PREFIX, location, |name| {
            components.request_bodies.get(name)
        })?;
        let Some((content_type, media)) = preferred_media_type(&body.content) else {
            self.issues.push(ValidationIssue::new(
                location,
                "request body declares no content types",
            ));
            return None;
        };
        let schema = media
            .schema
            .as_ref()
            .and_then(|schema| self.normalize(schema, location));
        Some(RequestBody {
            required: body.required,
            content_type: content_type.clone(),
            description: body.description.clone(),
            schema,
        })
    }

    fn security_schemes(&mut self) -> BTreeMap<String, SecurityScheme> {
        let mut schemes = BTreeMap::new();
        let components = self.components;
        for (name, reference) in &components.security_schemes {
            let location = format!("components.securitySchemes.{name}");
            if let Some(scheme) =
                self.resolve(reference, SECURITY_SCHEMES_PREFIX, &location, |name| {
                    components.security_schemes.get(name)
                })
            {
                schemes.insert(name.clone(), security_scheme(scheme));
            }
        }
        schemes
    }

    fn schemas(&mut self) -> BTreeMap<String, Value> {
        let mut schemas = BTreeMap::new();
        let components = self.components;
        for (name, schema) in &components.schemas {
            let location = format!("components.schemas.{name}");
            if let Some(json) = self.normalize(schema, &location) {
                schemas.insert(name.clone(), json);
            }
        }
        schemas
    }

    /// Follows local `$ref`s to a component, recording an issue when it can't.
    fn resolve<T>(
        &mut self,
        reference: &'a oa::ReferenceOr<T>,
        prefix: &str,
        location: &str,
        lookup: impl Fn(&str) -> Option<&'a oa::ReferenceOr<T>>,
    ) -> Option<&'a T> {
        let mut current = reference;
        for _ in 0..MAX_REFERENCE_DEPTH {
            let target = match current {
                oa::ReferenceOr::Item(item) => return Some(item),
                oa::ReferenceOr::Reference { reference } => reference,
            };
            let Some(name) = target.strip_prefix(prefix) else {
                self.issues.push(ValidationIssue::new(
                    location,
                    format!("reference `{target}` is not supported; only local `{prefix}` references are"),
                ));
                return None;
            };
            let Some(next) = lookup(name) else {
                self.issues.push(ValidationIssue::new(
                    location,
                    format!("reference `{target}` points to an undefined component"),
                ));
                return None;
            };
            current = next;
        }
        self.issues.push(ValidationIssue::new(
            location,
            "reference chain is circular or deeper than 16 levels",
        ));
        None
    }

    fn normalize(&mut self, value: &impl Serialize, location: &str) -> Option<Value> {
        serde_json::to_value(value)
            .inspect_err(|error| {
                self.issues.push(ValidationIssue::new(
                    location,
                    format!("schema could not be normalized: {error}"),
                ));
            })
            .ok()
    }
}

/// OpenAPI serialization styles for array values. Query and cookie
/// parameters repeat by default (`form`, explode); `explode: false` and the
/// delimited styles join; header and path values (`simple`) always join with commas.
fn array_delimiter(parameter: &oa::Parameter) -> Option<char> {
    const COMMA: char = ',';
    match parameter {
        oa::Parameter::Query {
            parameter_data,
            style,
            ..
        } => match style {
            oa::QueryStyle::SpaceDelimited => Some(' '),
            oa::QueryStyle::PipeDelimited => Some('|'),
            oa::QueryStyle::Form if parameter_data.explode == Some(false) => Some(COMMA),
            oa::QueryStyle::Form | oa::QueryStyle::DeepObject => None,
        },
        oa::Parameter::Cookie { parameter_data, .. } => {
            (parameter_data.explode == Some(false)).then_some(COMMA)
        }
        oa::Parameter::Header { .. } | oa::Parameter::Path { .. } => Some(COMMA),
    }
}

/// Accepts `"users orders"` or `["users", "orders"]`.
fn resource_words(value: &Value) -> Option<Vec<String>> {
    let words: Vec<String> = match value {
        Value::String(text) => text.split_whitespace().map(str::to_owned).collect(),
        Value::Array(items) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect::<Option<_>>()?,
        _ => return None,
    };
    if words.is_empty() { None } else { Some(words) }
}

/// Prefers JSON, then any `+json` type, then whatever is declared first.
fn preferred_media_type(content: &oa::Content) -> Option<(&String, &oa::MediaType)> {
    content
        .iter()
        .find(|(media_type, _)| media_type.as_str() == JSON_MEDIA_TYPE)
        .or_else(|| {
            content
                .iter()
                .find(|(media_type, _)| media_type.ends_with("+json"))
        })
        .or_else(|| content.iter().next())
}

fn server_url(server: &oa::Server) -> String {
    let mut url = server.url.clone();
    for (name, variable) in server.variables.iter().flatten() {
        url = url.replace(&format!("{{{name}}}"), &variable.default);
    }
    url
}

fn security_requirement(requirement: &oa::SecurityRequirement) -> SecurityRequirement {
    SecurityRequirement {
        schemes: requirement
            .iter()
            .map(|(scheme, scopes)| SchemeRequirement {
                scheme: scheme.clone(),
                scopes: scopes.clone(),
            })
            .collect(),
    }
}

fn security_scheme(scheme: &oa::SecurityScheme) -> SecurityScheme {
    match scheme {
        oa::SecurityScheme::APIKey { location, name, .. } => SecurityScheme::ApiKey {
            name: name.clone(),
            location: match location {
                oa::APIKeyLocation::Query => ApiKeyLocation::Query,
                oa::APIKeyLocation::Header => ApiKeyLocation::Header,
                oa::APIKeyLocation::Cookie => ApiKeyLocation::Cookie,
            },
        },
        oa::SecurityScheme::HTTP {
            scheme,
            bearer_format,
            ..
        } => match scheme.to_ascii_lowercase().as_str() {
            "basic" => SecurityScheme::HttpBasic,
            "bearer" => SecurityScheme::HttpBearer {
                bearer_format: bearer_format.clone(),
            },
            other => SecurityScheme::Unsupported {
                description: format!("HTTP authentication scheme `{other}`"),
            },
        },
        oa::SecurityScheme::OAuth2 { flows, .. } => oauth2(flows),
        oa::SecurityScheme::OpenIDConnect {
            open_id_connect_url,
            ..
        } => SecurityScheme::OpenIdConnect {
            url: open_id_connect_url.clone(),
        },
    }
}

fn oauth2(flows: &oa::OAuth2Flows) -> SecurityScheme {
    let mut supported = Vec::new();
    if let Some(flow) = &flows.client_credentials {
        supported.push(OAuth2Flow::ClientCredentials {
            token_url: flow.token_url.clone(),
            refresh_url: flow.refresh_url.clone(),
            scopes: scopes(&flow.scopes),
        });
    }
    if let Some(flow) = &flows.authorization_code {
        supported.push(OAuth2Flow::AuthorizationCode {
            authorization_url: flow.authorization_url.clone(),
            token_url: flow.token_url.clone(),
            refresh_url: flow.refresh_url.clone(),
            scopes: scopes(&flow.scopes),
        });
    }
    if supported.is_empty() {
        return SecurityScheme::Unsupported {
            description: "OAuth2 implicit and password flows".to_owned(),
        };
    }
    SecurityScheme::OAuth2 { flows: supported }
}

fn scopes<'s>(
    scopes: impl IntoIterator<Item = (&'s String, &'s String)>,
) -> BTreeMap<String, String> {
    scopes
        .into_iter()
        .map(|(name, description)| (name.clone(), description.clone()))
        .collect()
}
