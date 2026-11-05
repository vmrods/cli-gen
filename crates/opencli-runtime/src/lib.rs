//! Request planning and execution for OpenCLI commands.

mod plan;
mod send;

pub use plan::{PlanError, PlannedBody, RequestPlan, plan_request, typed_value};
pub use send::{DEFAULT_TIMEOUT, HttpClient, Response, SendError};
