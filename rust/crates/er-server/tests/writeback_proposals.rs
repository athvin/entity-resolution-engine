//! Pure proposal invariants, before persistence or any provider call exists.
use std::collections::{BTreeMap, BTreeSet};

use er_server::writeback::{
    MemberSnapshot, MergeObject, MergeProposal, ProposalError, ProposalInput, ProposalScope,
    ProviderScope,
};
use serde_json::{Value, json};

fn fields(values: &[(&str, Value)]) -> BTreeMap<String, Value> {
    values
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect()
}

fn input() -> ProposalInput {
    let provider = ProviderScope {
        org_id: "org-a".into(),
        dataset_id: "dataset-a".into(),
        connection_id: "connection-a".into(),
        object: MergeObject::Contact,
    };
    ProposalInput {
        scope: ProposalScope {
            provider: provider.clone(),
            entity_id: "entity-a".into(),
            lake_snapshot: 42,
            policy_revision: "revision-a".into(),
            election_policy: "most_attributes".into(),
        },
        master_record_key: "b".into(),
        members: ["c", "b", "a"]
            .map(|key| MemberSnapshot {
                provider: provider.clone(),
                record_key: key.into(),
                external_id: format!("external-{key}"),
                system_modstamp: "2026-11-01T12:00:00.000Z".into(),
                fields: fields(&[
                    ("FirstName", json!("Original")),
                    ("Email", json!("old@example.test")),
                    ("Phone", Value::Null),
                ]),
            })
            .into(),
        golden_fields: fields(&[
            ("given_name", json!("Golden")),
            ("email", Value::Null),
            ("phone", Value::Null),
        ]),
        field_mapping: [
            ("given_name".into(), "FirstName".into()),
            ("email".into(), "Email".into()),
            ("phone".into(), "Phone".into()),
        ]
        .into(),
        writeback_allowlist: ["FirstName".into(), "Email".into(), "Phone".into()].into(),
    }
}

#[test]
fn preserves_the_elected_master_preimages_and_explicit_null_changes() {
    let proposal = MergeProposal::build(input()).unwrap();
    let content = proposal.content();
    assert_eq!(content.master.record_key, "b");
    assert_eq!(content.victims[0].record_key, "a");
    assert_eq!(content.victims[1].record_key, "c");
    assert_eq!(content.master.fields["FirstName"], "Original");
    assert_eq!(content.field_changes["FirstName"].after, "Golden");
    assert_eq!(content.field_changes["Email"].before, "old@example.test");
    assert_eq!(content.field_changes["Email"].after, Value::Null);
    assert!(!content.field_changes.contains_key("Phone"));
    assert_eq!(proposal.fingerprint().len(), 67);
    assert!(proposal.fingerprint().starts_with("v1:"));
}

#[test]
fn missing_golden_and_disallowed_fields_never_propose_clearing() {
    let mut candidate = input();
    candidate.golden_fields.remove("email");
    candidate.writeback_allowlist.remove("FirstName");
    assert!(
        MergeProposal::build(candidate)
            .unwrap()
            .content()
            .field_changes
            .is_empty()
    );
    let mut candidate = input();
    candidate.writeback_allowlist = BTreeSet::new();
    assert!(
        MergeProposal::build(candidate)
            .unwrap()
            .content()
            .field_changes
            .is_empty()
    );
}

#[test]
fn scalar_values_keep_their_types_and_empty_is_not_null() {
    let mut candidate = input();
    for (attribute, field, before, after) in [
        ("flag", "Flag__c", json!(true), json!(false)),
        ("count", "Count__c", json!(5), json!(0)),
        ("note", "Note__c", Value::Null, json!("")),
    ] {
        candidate
            .field_mapping
            .insert(attribute.into(), field.into());
        candidate.writeback_allowlist.insert(field.into());
        candidate.members[1].fields.insert(field.into(), before);
        candidate.golden_fields.insert(attribute.into(), after);
    }
    let proposal = MergeProposal::build(candidate).unwrap();
    let changes = &proposal.content().field_changes;
    assert_eq!(changes["Flag__c"].after, json!(false));
    assert_eq!(changes["Count__c"].after, json!(0));
    assert_eq!(changes["Note__c"].before, Value::Null);
    assert_eq!(changes["Note__c"].after, json!(""));
}

#[test]
fn every_member_permutation_has_identical_content_and_fingerprint() {
    let original = input();
    let expected = serde_json::to_vec(&MergeProposal::build(original.clone()).unwrap()).unwrap();
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let mut candidate = original.clone();
        candidate.members = order.map(|i| original.members[i].clone()).into();
        let actual = serde_json::to_vec(&MergeProposal::build(candidate).unwrap()).unwrap();
        assert_eq!(actual, expected);
    }
}

fn changed_identity(change: impl FnOnce(&mut ProposalInput)) {
    let mut candidate = input();
    change(&mut candidate);
    let changed = MergeProposal::build(candidate).unwrap();
    assert_ne!(
        changed.fingerprint(),
        MergeProposal::build(input()).unwrap().fingerprint()
    );
}

#[test]
fn fingerprint_binds_scope_policy_membership_revisions_preimages_and_changes() {
    for key in ["org", "dataset", "connection", "object"] {
        changed_identity(|p| {
            let scope = &mut p.scope.provider;
            match key {
                "org" => scope.org_id = "org-b".into(),
                "dataset" => scope.dataset_id = "dataset-b".into(),
                "connection" => scope.connection_id = "connection-b".into(),
                _ => scope.object = MergeObject::Lead,
            }
            for member in &mut p.members {
                member.provider = scope.clone();
            }
        });
    }
    changed_identity(|p| p.scope.entity_id = "entity-b".into());
    changed_identity(|p| p.scope.lake_snapshot += 1);
    changed_identity(|p| p.scope.policy_revision = "revision-b".into());
    changed_identity(|p| p.scope.election_policy = "oldest".into());
    changed_identity(|p| p.master_record_key = "a".into());
    changed_identity(|p| {
        p.members.pop();
    });
    changed_identity(|p| p.members[0].external_id = "external-d".into());
    changed_identity(|p| p.members[0].record_key = "d".into());
    changed_identity(|p| p.members[0].system_modstamp = "2026-11-02T12:00:00.000Z".into());
    changed_identity(|p| {
        p.members[0].fields.insert("Phone".into(), json!("123"));
    });
    changed_identity(|p| {
        p.members[1]
            .fields
            .insert("Email".into(), json!("changed@example.test"));
    });
    changed_identity(|p| {
        p.golden_fields
            .insert("given_name".into(), json!("Changed"));
    });
    changed_identity(|p| {
        p.writeback_allowlist.remove("Email");
    });
    changed_identity(|p| {
        p.field_mapping.remove("email");
    });
}

#[test]
fn rejects_cross_org_dataset_connection_and_object_members() {
    for key in ["org", "dataset", "connection", "object"] {
        let mut candidate = input();
        let scope = &mut candidate.members[0].provider;
        match key {
            "org" => scope.org_id = "org-b".into(),
            "dataset" => scope.dataset_id = "dataset-b".into(),
            "connection" => scope.connection_id = "connection-b".into(),
            _ => scope.object = MergeObject::Account,
        }
        assert!(matches!(
            MergeProposal::build(candidate),
            Err(ProposalError::MixedScope)
        ));
    }
}

#[test]
fn rejects_duplicate_keys_and_external_ids() {
    let mut candidate = input();
    candidate.members[0].record_key = "b".into();
    assert!(matches!(
        MergeProposal::build(candidate),
        Err(ProposalError::DuplicateMember)
    ));
    let mut candidate = input();
    candidate.members[0].external_id = "external-b".into();
    assert!(matches!(
        MergeProposal::build(candidate),
        Err(ProposalError::DuplicateMember)
    ));
}

#[test]
fn rejects_missing_master_and_singletons() {
    let mut candidate = input();
    candidate.master_record_key = "not-a-member".into();
    assert!(matches!(
        MergeProposal::build(candidate),
        Err(ProposalError::MissingMaster)
    ));
    let mut candidate = input();
    candidate.members.truncate(1);
    assert!(matches!(
        MergeProposal::build(candidate),
        Err(ProposalError::TooFewMembers)
    ));
}

#[test]
fn rejects_missing_stamps_identity_invalid_snapshot_and_unknown_policy() {
    let mut candidate = input();
    candidate.members[0].system_modstamp = " ".into();
    assert!(matches!(
        MergeProposal::build(candidate),
        Err(ProposalError::MissingIdentity)
    ));
    let mut candidate = input();
    candidate.scope.provider.org_id.clear();
    assert!(matches!(
        MergeProposal::build(candidate),
        Err(ProposalError::MissingIdentity)
    ));
    let mut candidate = input();
    candidate.scope.lake_snapshot = -1;
    assert!(matches!(
        MergeProposal::build(candidate),
        Err(ProposalError::InvalidSnapshot)
    ));
    let mut candidate = input();
    candidate.scope.election_policy = "invented".into();
    assert!(matches!(
        MergeProposal::build(candidate),
        Err(ProposalError::UnknownElectionPolicy)
    ));
}

#[test]
fn rejects_missing_master_preimage_instead_of_treating_it_as_null() {
    let mut candidate = input();
    candidate.members[1].fields.remove("Email");
    assert!(matches!(
        MergeProposal::build(candidate),
        Err(ProposalError::MissingMasterField)
    ));
}

#[test]
fn rejects_ambiguous_mapping_and_protected_writes_even_when_disallowed() {
    let mut candidate = input();
    candidate
        .field_mapping
        .insert("another_name".into(), "firstname".into());
    assert!(matches!(
        MergeProposal::build(candidate),
        Err(ProposalError::DuplicateMappingTarget)
    ));
    for target in ["Id", "SYSTEMMODSTAMP"] {
        let mut candidate = input();
        candidate.field_mapping.insert("bad".into(), target.into());
        candidate.writeback_allowlist.clear();
        assert!(matches!(
            MergeProposal::build(candidate),
            Err(ProposalError::ProtectedField)
        ));
        let mut candidate = input();
        candidate.writeback_allowlist.insert(target.into());
        assert!(matches!(
            MergeProposal::build(candidate),
            Err(ProposalError::ProtectedField)
        ));
    }
}

#[test]
fn rejects_nested_values_including_victim_preimages_and_unused_golden_fields() {
    for value in [json!([1, 2]), json!({"nested": "sensitive"})] {
        let mut candidate = input();
        candidate.members[0]
            .fields
            .insert("Notes".into(), value.clone());
        assert!(matches!(
            MergeProposal::build(candidate),
            Err(ProposalError::NonScalarField)
        ));
        let mut candidate = input();
        candidate.golden_fields.insert("unused".into(), value);
        let error = MergeProposal::build(candidate).err().unwrap();
        assert!(matches!(error, ProposalError::NonScalarField));
        assert!(!error.to_string().contains("sensitive"));
    }
}

#[test]
fn rejects_malformed_provider_field_names() {
    for field in ["", "Account.Name", "Name; DROP", "1name", "Náme"] {
        let mut candidate = input();
        candidate
            .field_mapping
            .insert("attribute".into(), field.into());
        assert!(matches!(
            MergeProposal::build(candidate),
            Err(ProposalError::InvalidFieldName)
        ));
    }
}
