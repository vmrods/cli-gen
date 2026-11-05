//! Request execution for OpenCLI commands. Planning is pure; sending arrives in M3.

mod plan;

pub use plan::{PlanError, PlannedBody, RequestPlan, plan_request, typed_value};
