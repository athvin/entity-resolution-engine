//! Print either API audience. The default internal document feeds schema.d.ts.
use clap::Parser;
use er_server::openapi::Audience;

#[derive(Parser)]
struct Args {
    #[arg(long, value_enum, default_value = "internal")]
    audience: Audience,
}

fn main() {
    print!(
        "{}",
        er_server::openapi::openapi_json(Args::parse().audience)
    );
}
