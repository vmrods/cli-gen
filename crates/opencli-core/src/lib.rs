//! Domain model shared by every OpenCLI crate. Free of I/O and parser types.

mod api_name;
mod spec;

pub use api_name::{ApiName, ApiNameError, RESERVED_NAMES};
pub use spec::{
    ApiKeyLocation, ApiSpec, HttpMethod, OAuth2Flow, Operation, Parameter, ParameterLocation,
    RequestBody, SchemeRequirement, SecurityRequirement, SecurityScheme, SpecFormat,
    UnknownVariant,
};
