//! Domain model shared by every OpenCLI crate. Free of I/O and parser types.

mod api_name;
mod body;
mod commands;
mod diff;
mod naming;
mod schema;
mod spec;

pub use api_name::{ApiName, ApiNameError, RESERVED_NAMES};
pub use body::{BodyProblem, is_secret_field, validate_body};
pub use commands::{
    ArgumentSpec, ArgumentTarget, BodySpec, CommandSet, CommandSpec, NamingConflict, NamingError,
    ValueType, build_commands,
};
pub use diff::{CommandChange, CommandDiff, CommandRename, CommandUpdate, diff_commands};
pub use naming::kebab_case;
pub use spec::{
    ApiKeyLocation, ApiSpec, CliHints, FORM_MEDIA_TYPE, HttpMethod, MULTIPART_MEDIA_TYPE,
    OAuth2Flow, Operation, Parameter, ParameterLocation, RequestBody, SchemeRequirement,
    SecurityRequirement, SecurityScheme, SpecFormat, UnknownVariant, is_form_media_type,
    is_json_media_type, is_multipart_media_type,
};
