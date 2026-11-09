//! Pure derivation of the API contract from handler/router source text.
//!
//! These routines preserve the existing MCP, envelope and mock-contract gates.
//! Callers provide source strings; this crate performs no file or network I/O and
//! does not depend on the server. Unknown envelope shapes remain explicit errors.

mod envelopes;
mod manifest;
mod operations;
mod routes;

#[cfg(test)]
mod tests;

pub use envelopes::{
    Envelope, classify_envelope, envelope_ledger, handler_bodies, object_keys, success_only,
};
pub use manifest::{
    Field, Tool, body_schema, body_type_after, derive_tools, manifest_json, method_and_path,
    path_params, role_of, struct_fields, summary, tool_name, type_schema,
};
pub use operations::{Operation, Parameter, Request, operations};
pub use routes::{RouteRegistration, get_handler_in, get_routes, routes, verbs_in};
