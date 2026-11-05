//! Request planning and execution for OpenCLI commands.

mod plan;
mod send;
mod transport;

pub use plan::{
    CredentialLocation, CredentialPlacement, PlanError, PlannedBody, RequestPlan, plan_request,
    typed_value,
};
pub use send::{DEFAULT_TIMEOUT, HttpClient, Response, SendError};
pub use transport::{is_loopback, is_secure_transport};
