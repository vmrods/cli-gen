//! Domain model shared by every OpenCLI crate. Free of I/O and parser types.

mod api_name;
mod commands;
mod naming;
mod schema;
mod spec;

pub use api_name::{ApiName, ApiNameError, RESERVED_NAMES};
pub use commands::{
    ArgumentSpec, ArgumentTarget, BodySpec, CommandSet, CommandSpec, NamingConflict, NamingError,
    ValueType, build_commands,
};
pub use naming::kebab_case;
pub use spec::{
    ApiKeyLocation, ApiSpec, CliHints, HttpMethod, OAuth2Flow, Operation, Parameter,
    ParameterLocation, RequestBody, SchemeRequirement, SecurityRequirement, SecurityScheme,
    SpecFormat, UnknownVariant, is_json_media_type,
};
