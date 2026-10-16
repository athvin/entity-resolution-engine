//! Print the control-lane OpenAPI document to stdout. The committed
//! `frontend/src/lib/api/openapi.json` is this output; the snapshot test in
//! `er_server::openapi` keeps them in lock-step. Regenerate with:
//!   cargo run -p er-server --bin er-openapi > frontend/src/lib/api/openapi.json
//! then `pnpm gen:api` to refresh `schema.d.ts`.

fn main() {
    print!("{}", er_server::openapi::openapi_json());
}
