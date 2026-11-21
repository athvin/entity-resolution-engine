//! Backend-independent execution of generated Arrow reads. Parameter values must
//! never be formatted into diagnostics or telemetry.

use arrow::record_batch::RecordBatch;

/// An owned, typed bind value. There is deliberately no Debug implementation.
/// Text also carries the generated SQL's explicitly cast JSON/date/timestamp
/// values. Lists remain typed locally; transports may refuse unsupported types.
#[derive(Clone)]
pub enum Parameter {
    Null,
    Boolean(bool),
    Bigint(i64),
    Double(f64),
    Text(String),
    ListVarchar(Vec<String>),
}

/// The small execution seam used by generated snapshot reads. Implementations
/// must retain one session across calls and must never replay a failed query or
/// replace a lost session. Lifecycle and cancellation belong to the owner.
pub trait Executor {
    type Error: std::fmt::Display;

    fn query(&self, sql: &str, parameters: &[Parameter]) -> Result<Vec<RecordBatch>, Self::Error>;
}

impl Executor for duckdb::Connection {
    type Error = duckdb::Error;

    fn query(&self, sql: &str, parameters: &[Parameter]) -> Result<Vec<RecordBatch>, Self::Error> {
        use duckdb::types::Value;
        let values: Vec<Value> = parameters
            .iter()
            .map(|parameter| match parameter {
                Parameter::Null => Value::Null,
                Parameter::Boolean(value) => Value::Boolean(*value),
                Parameter::Bigint(value) => Value::BigInt(*value),
                Parameter::Double(value) => Value::Double(*value),
                Parameter::Text(value) => Value::Text(value.clone()),
                Parameter::ListVarchar(values) => {
                    Value::List(values.iter().cloned().map(Value::Text).collect())
                }
            })
            .collect();
        self.prepare(sql)?
            .query_arrow(duckdb::params_from_iter(values))
            .map(Iterator::collect)
    }
}
