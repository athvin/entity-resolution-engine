//! Immutable proposals at the boundary between golden records and writeback.
//!
//! This module does no I/O and grants no authorization. The materialization job
//! must resolve scope, read a pinned snapshot, and elect a master through
//! `er_golden::master` over the eligible connection/object group before calling
//! [`MergeProposal::build`]. The existing cross-source export's master is not
//! necessarily eligible. Approval, persistence and provider calls live above
//! this boundary (E9 children #993–#998).
//!
//! Record fields and snapshots contain customer data: do not log the inputs or
//! serialized proposal. Errors deliberately carry only structural categories.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The Salesforce objects covered by the v1 merge design.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeObject {
    Lead,
    Contact,
    Account,
}

/// Scope resolved by the caller, never inferred from an external record ID.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderScope {
    pub org_id: String,
    pub dataset_id: String,
    pub connection_id: String,
    pub object: MergeObject,
}

/// The exact source and policy revision a future approval will bind.
#[derive(Clone, Serialize, Deserialize)]
pub struct ProposalScope {
    pub provider: ProviderScope,
    pub entity_id: String,
    pub lake_snapshot: i64,
    pub policy_revision: String,
    /// One of `er_golden::MASTER_ELECTION_POLICIES`; election stays there.
    pub election_policy: String,
}

/// A member's synchronized preimage, including fields needed for restore.
#[derive(Clone, Serialize, Deserialize)]
pub struct MemberSnapshot {
    pub provider: ProviderScope,
    pub record_key: String,
    pub external_id: String,
    /// Provider revision token, preserved verbatim for the apply staleness check.
    pub system_modstamp: String,
    /// Provider field names and scalar JSON values; absent is not null.
    pub fields: BTreeMap<String, Value>,
}

/// Unvalidated inputs. There is intentionally no deserializer for a validated
/// proposal: restoring persisted data must pass the builder again.
#[derive(Clone, Serialize, Deserialize)]
pub struct ProposalInput {
    pub scope: ProposalScope,
    pub master_record_key: String,
    pub members: Vec<MemberSnapshot>,
    /// Engine attribute names, not provider field names.
    pub golden_fields: BTreeMap<String, Value>,
    /// Engine attribute -> provider field. Targets must be unique.
    pub field_mapping: BTreeMap<String, String>,
    /// Explicit provider field allowlist. Empty means no field updates.
    pub writeback_allowlist: BTreeSet<String>,
}

/// A reviewable field change. Explicit null in `after` proposes clearing it.
#[derive(Clone, PartialEq, Serialize)]
pub struct FieldChange {
    pub before: Value,
    pub after: Value,
}

/// Canonical content. Borrowed from a proposal; no mutable access is exposed.
#[derive(Clone, Serialize)]
pub struct ProposalContent {
    pub scope: ProposalScope,
    pub master: MemberSnapshot,
    /// Ordered by record key, independent of the lake's row order.
    pub victims: Vec<MemberSnapshot>,
    pub field_changes: BTreeMap<String, FieldChange>,
    pub field_mapping: BTreeMap<String, String>,
    pub writeback_allowlist: BTreeSet<String>,
}

/// Validated content and its versioned, scoped identity. A fingerprint is an
/// identity, not a signature, a permission grant, or evidence of freshness.
#[derive(Clone, Serialize)]
pub struct MergeProposal {
    content: ProposalContent,
    fingerprint: String,
}

/// Structural failures, with no record values or identifiers in their display.
#[derive(Debug, thiserror::Error)]
pub enum ProposalError {
    #[error("proposal identity or revision is missing")]
    MissingIdentity,
    #[error("proposal lake snapshot must be nonnegative")]
    InvalidSnapshot,
    #[error("unknown master election policy")]
    UnknownElectionPolicy,
    #[error("a merge proposal requires at least two members")]
    TooFewMembers,
    #[error("member scope differs from proposal scope")]
    MixedScope,
    #[error("duplicate member record key or external id")]
    DuplicateMember,
    #[error("the elected master is not a member of this group")]
    MissingMaster,
    #[error("provider field name is invalid")]
    InvalidFieldName,
    #[error("field snapshots must contain only scalar JSON values")]
    NonScalarField,
    #[error("multiple golden attributes map to the same provider field")]
    DuplicateMappingTarget,
    #[error("identity and revision fields cannot be writeback targets")]
    ProtectedField,
    #[error("an eligible mapped master field is missing from the snapshot")]
    MissingMasterField,
    #[error("serializing proposal identity")]
    Serialize(#[from] serde_json::Error),
}

impl MergeProposal {
    /// Validate, sort, compute explicit diffs, and fingerprint owned content.
    ///
    /// Missing golden attributes are skipped; they never mean clearing a field.
    /// A present null is a value, and can propose a clear only when allowlisted.
    /// The builder does not check Salesforce permissions or current metadata.
    ///
    /// # Errors
    /// Returns [`ProposalError`] for invalid identity/scope, membership, mapping
    /// or snapshots. No partial proposal is returned.
    pub fn build(mut input: ProposalInput) -> Result<Self, ProposalError> {
        validate(&input)?;
        input
            .members
            .sort_by(|a, b| a.record_key.cmp(&b.record_key));
        let master_index = input
            .members
            .iter()
            .position(|member| member.record_key == input.master_record_key)
            .ok_or(ProposalError::MissingMaster)?;
        let master = input.members.remove(master_index);
        let mut field_changes = BTreeMap::new();
        for (attribute, field) in &input.field_mapping {
            if !input.writeback_allowlist.contains(field) {
                continue;
            }
            let Some(after) = input.golden_fields.get(attribute) else {
                continue;
            };
            let before = master
                .fields
                .get(field)
                .ok_or(ProposalError::MissingMasterField)?;
            if before != after {
                field_changes.insert(
                    field.clone(),
                    FieldChange {
                        before: before.clone(),
                        after: after.clone(),
                    },
                );
            }
        }
        let content = ProposalContent {
            scope: input.scope,
            master,
            victims: input.members,
            field_changes,
            field_mapping: input.field_mapping,
            writeback_allowlist: input.writeback_allowlist,
        };
        // Every map is ordered and every value scalar, including victim
        // preimages. Domain separation makes future format changes explicit.
        let bytes = serde_json::to_vec(&content)?;
        let mut hash = Sha256::new();
        hash.update(b"er-writeback-proposal-v1\0");
        hash.update(bytes);
        let fingerprint = format!("v1:{}", hex::encode(hash.finalize()));
        Ok(Self {
            content,
            fingerprint,
        })
    }

    #[must_use]
    pub fn content(&self) -> &ProposalContent {
        &self.content
    }

    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

fn require_identity(value: &str) -> Result<(), ProposalError> {
    if value.trim().is_empty() {
        return Err(ProposalError::MissingIdentity);
    }
    Ok(())
}

fn validate_field_name(field: &str) -> Result<(), ProposalError> {
    let mut bytes = field.bytes();
    if !bytes.next().is_some_and(|b| b.is_ascii_alphabetic())
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(ProposalError::InvalidFieldName);
    }
    Ok(())
}

fn validate_target(field: &str) -> Result<(), ProposalError> {
    validate_field_name(field)?;
    if field.eq_ignore_ascii_case("Id") || field.eq_ignore_ascii_case("SystemModstamp") {
        return Err(ProposalError::ProtectedField);
    }
    Ok(())
}

fn validate_scalars(fields: &BTreeMap<String, Value>) -> Result<(), ProposalError> {
    if fields.values().any(|v| v.is_array() || v.is_object()) {
        return Err(ProposalError::NonScalarField);
    }
    Ok(())
}

fn validate(input: &ProposalInput) -> Result<(), ProposalError> {
    let scope = &input.scope;
    for identity in [
        &scope.provider.org_id,
        &scope.provider.dataset_id,
        &scope.provider.connection_id,
        &scope.entity_id,
        &scope.policy_revision,
        &input.master_record_key,
    ] {
        require_identity(identity)?;
    }
    if scope.lake_snapshot < 0 {
        return Err(ProposalError::InvalidSnapshot);
    }
    if !er_golden::MASTER_ELECTION_POLICIES.contains(&scope.election_policy.as_str()) {
        return Err(ProposalError::UnknownElectionPolicy);
    }
    if input.members.len() < 2 {
        return Err(ProposalError::TooFewMembers);
    }
    let mut keys = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for member in &input.members {
        if member.provider != scope.provider {
            return Err(ProposalError::MixedScope);
        }
        require_identity(&member.record_key)?;
        require_identity(&member.external_id)?;
        require_identity(&member.system_modstamp)?;
        if !keys.insert(&member.record_key) || !ids.insert(&member.external_id) {
            return Err(ProposalError::DuplicateMember);
        }
        for field in member.fields.keys() {
            validate_field_name(field)?;
        }
        validate_scalars(&member.fields)?;
    }
    validate_scalars(&input.golden_fields)?;
    let mut targets = BTreeSet::new();
    for (attribute, target) in &input.field_mapping {
        require_identity(attribute)?;
        validate_target(target)?;
        // Conservatively reject differently cased aliases too. The caller
        // must supply canonical provider field names from its mapping.
        if !targets.insert(target.to_ascii_lowercase()) {
            return Err(ProposalError::DuplicateMappingTarget);
        }
    }
    for field in &input.writeback_allowlist {
        validate_target(field)?;
    }
    Ok(())
}

/// The persisted form of a proposal (#1305): its UNVALIDATED input and a
/// format version. A validated [`MergeProposal`] is never deserialized —
/// restoring one always runs [`MergeProposal::build`] again, so stored bytes
/// cannot smuggle in a proposal the builder would refuse, and the rebuilt
/// fingerprint must equal the one an approval names.
#[derive(Clone, Serialize, Deserialize)]
pub struct PersistedProposalV1 {
    pub format_version: u32,
    pub input: ProposalInput,
}

impl PersistedProposalV1 {
    pub const FORMAT_VERSION: u32 = 1;

    #[must_use]
    pub fn new(input: ProposalInput) -> Self {
        Self {
            format_version: Self::FORMAT_VERSION,
            input,
        }
    }

    /// Rebuild the validated proposal.
    ///
    /// # Errors
    /// An unknown format version, or anything the builder refuses.
    pub fn restore(value: &Value) -> Result<MergeProposal, RestoreError> {
        let persisted: Self = serde_json::from_value(value.clone())?;
        if persisted.format_version != Self::FORMAT_VERSION {
            return Err(RestoreError::UnknownFormat(persisted.format_version));
        }
        Ok(MergeProposal::build(persisted.input)?)
    }
}

/// Why a persisted proposal could not be restored.
#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("persisted proposal is malformed")]
    Malformed(#[from] serde_json::Error),
    #[error("unknown persisted proposal format {0}")]
    UnknownFormat(u32),
    #[error("persisted proposal no longer validates")]
    Invalid(#[from] ProposalError),
}

#[cfg(test)]
mod persisted_tests {
    use super::*;

    fn input() -> ProposalInput {
        let provider = ProviderScope {
            org_id: "org".into(),
            dataset_id: "ds".into(),
            connection_id: "conn".into(),
            object: MergeObject::Contact,
        };
        let member = |key: &str, id: &str, email: Value| MemberSnapshot {
            provider: provider.clone(),
            record_key: key.into(),
            external_id: id.into(),
            system_modstamp: "2026-11-27T00:00:00.000+0000".into(),
            fields: BTreeMap::from([
                ("Email".to_string(), email),
                ("Phone".to_string(), Value::Null),
            ]),
        };
        ProposalInput {
            scope: ProposalScope {
                provider: provider.clone(),
                entity_id: "e1".into(),
                lake_snapshot: 7,
                policy_revision: "rev1".into(),
                election_policy: er_golden::DEFAULT_MASTER_ELECTION_POLICY.into(),
            },
            master_record_key: "crm:a".into(),
            members: vec![
                member("crm:b", "003B", Value::String("b@x.test".into())),
                member("crm:a", "003A", Value::String("a@x.test".into())),
            ],
            golden_fields: BTreeMap::from([
                ("email".to_string(), Value::String("b@x.test".into())),
                ("phone".to_string(), Value::Null),
            ]),
            field_mapping: BTreeMap::from([
                ("email".to_string(), "Email".to_string()),
                ("phone".to_string(), "Phone".to_string()),
            ]),
            writeback_allowlist: BTreeSet::from(["Email".to_string(), "Phone".to_string()]),
        }
    }

    #[test]
    fn a_persisted_proposal_restores_to_the_same_fingerprint() {
        let built = MergeProposal::build(input()).unwrap();
        let stored = serde_json::to_value(PersistedProposalV1::new(input())).unwrap();
        let restored = PersistedProposalV1::restore(&stored).unwrap();
        assert_eq!(restored.fingerprint(), built.fingerprint());
        // Pinned: a change to the canonical bytes is a format change and must
        // bump the version rather than silently re-fingerprint stored rows.
        assert_eq!(
            built.fingerprint(),
            PINNED_FINGERPRINT,
            "the v1 proposal bytes changed"
        );
        // An explicit null on the golden side equals the master's null: no change.
        assert!(!built.content().field_changes.contains_key("Phone"));
        assert_eq!(
            built.content().field_changes["Email"].after,
            Value::String("b@x.test".into())
        );
    }

    #[test]
    fn restoring_refuses_an_unknown_format_and_an_invalid_input() {
        let mut stored = serde_json::to_value(PersistedProposalV1::new(input())).unwrap();
        stored["format_version"] = Value::from(2);
        assert!(matches!(
            PersistedProposalV1::restore(&stored),
            Err(RestoreError::UnknownFormat(2))
        ));
        let mut bad = input();
        bad.master_record_key = "crm:missing".into();
        let stored = serde_json::to_value(PersistedProposalV1::new(bad)).unwrap();
        assert!(matches!(
            PersistedProposalV1::restore(&stored),
            Err(RestoreError::Invalid(_))
        ));
    }

    const PINNED_FINGERPRINT: &str =
        "v1:8d4057cacf51f12f5c215dabedc7e5880ba7b7171cf5576abf1189daca77ca8b";
}
