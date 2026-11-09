//! V19 — public JSON projections must not publish secrets or hide identity.

use crate::{Code, EntityRule, RuleInfo, Severity, Violation};
use er_entity_spec::{Entity, Visibility, Wire};

/// V19 — secrecy is declared once, at the column that every emitter consumes.
pub struct JsonVisibility;

impl EntityRule for JsonVisibility {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("V19"),
            title: "JSON secrets are internal and identity columns remain public",
            explain: "A JSON wire type, its OpenAPI schema and its TypeScript binding share \
                      column visibility. Secret-shaped names (secret, *_secret, token_hash, \
                      key_hash, password, *_token) must explicitly be internal so a new \
                      credential cannot silently become a public property. A key or natural_key \
                      column must stay public: a resource cannot hide its identity. This rule \
                      applies to JSON entities; storage rows and Arrow schemas retain all columns. \
                      Names are a guard, not a classifier for arbitrary sensitive payloads: \
                      authors must also mark other sensitive JSON columns internal.",
            conforming: "generate:\n  wire: [json]\nkey: [id]\ncolumns:\n  - { name: id, visibility: public }\n  - { name: secret, visibility: internal }\n",
            violating: "generate:\n  wire: [json]\nkey: [id]\ncolumns:\n  - { name: id, visibility: internal }\n  - { name: secret }\n",
            severity: Severity::Error,
        }
    }

    fn check(&self, entity: &Entity) -> Vec<Violation> {
        if !entity.has_wire(Wire::Json) {
            return Vec::new();
        }
        let mut violations = Vec::new();
        for column in &entity.columns {
            let name = column.name.as_str();
            let secret = matches!(name, "secret" | "token_hash" | "key_hash" | "password")
                || name.ends_with("_secret")
                || name.ends_with("_token");
            if secret && column.visibility != Visibility::Internal {
                violations.push(Violation::new(&entity.name, format!(
                    "secret-shaped column {name:?} on the JSON wire must declare visibility: internal"
                )));
            }
            if column.visibility == Visibility::Internal
                && (entity.key.contains(&column.name) || entity.natural_key.contains(&column.name))
            {
                violations.push(Violation::new(
                    &entity.name,
                    format!(
                        "identity column {name:?} (key/natural_key) must remain visibility: public"
                    ),
                ));
            }
        }
        violations
    }
}
