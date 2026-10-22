# The AWS SDK surface, and Bedrock's two halves

## Contents

- One crate has AWS dependencies, and its feature pins are load-bearing
- `aws sso login` does not work for a locally-run `er-server`
- Bedrock: permission is applied, model access is not
- Nothing in the workspace calls Bedrock today
- S3 is reached by DuckDB, not by the SDK
- The one long-lived access key

## One crate has AWS dependencies, and its feature pins are load-bearing

`er-server` is the **only** crate in the workspace with AWS dependencies:

```toml
aws-config            = { version = "1.8.11", default-features = false, features = ["rt-tokio", "behavior-version-latest"] }
aws-sdk-sesv2         = { version = "1.85.0", default-features = false, features = ["rt-tokio", "behavior-version-latest"] }
aws-smithy-http-client = { version = "1.1.4", default-features = false, features = ["rustls-ring"] }
```

`default-features = false` on all three is **not** tidiness, and relaxing it
breaks the build rather than merely adding weight. Two independent reasons,
both recorded in `Cargo.toml` beside the deps:

1. The default feature set pulls `aws-lc-rs`, whose `-sys` crate needs `cmake`
   that the `rust:1-bookworm` build image does not have. `rustls-ring` is the
   deliberate substitute — see the rust-conventions skill for the workspace TLS
   policy this is part of.
2. The defaults add credential providers the server cannot use anyway. It
   authenticates by **EKS Pod Identity**, which is the container-credentials
   provider already in the base chain.

Adding `aws-sdk-s3`, `aws-sdk-sts` or any other AWS crate means repeating the
same `default-features = false` + explicit-features shape. Use `cargo add` with
`--no-default-features` rather than hand-editing, per the repo convention.

## `aws sso login` does not work for a locally-run `er-server`

This follows directly from reason 2 above: the `sso` and `credentials-process`
providers are excluded, so a developer who has run `aws sso login` still has an
`er-server` that cannot authenticate to SES.

That is intended. Dev wants `ER_MAIL_BACKEND=log`, which is what the absent
credential degrades to. A locally-run server failing to send real email is the
correct behaviour, not a setup problem to debug.

(The excluded providers also require Rust 1.88, which was a second reason to
leave them out — though the workspace floor is 1.88 regardless now, after
`time`'s RUSTSEC-2026-0009 fix forced it.)

## Bedrock: permission is applied, model access is not

`infra/terraform/envs/dev/bedrock.tf` separates two things that are easy to
conflate, and only one of them is a decision:

| Half | What it is | State |
|---|---|---|
| **Permission** | the IAM policy letting the `er-api` pod invoke Claude, plus the Pod Identity binding that delivers it | applies cleanly as soon as a namespace is listed; no cost, nothing outward-facing |
| **Model access** | the account accepting Anthropic's terms — a customer-profile form plus acceptance of a **priced** offer | gated on `bedrock_use_case_form` being supplied, because both are the account owner's to make |

Until the second half is done, `converse` answers `Operation not allowed`.

One detail that changes what the policy must say: the current Claude models have
**no single-region endpoint**, so what gets invoked is a cross-region
*inference profile*, not a bare model id. The IAM resource patterns are
therefore `us.anthropic.claude-*` and `global.anthropic.claude-*`. A policy
written against a bare model ARN will not work.

## Nothing in the workspace calls Bedrock today

There is **no `aws-sdk-bedrockruntime` dependency anywhere**, and no Rust source
references Bedrock. The AI features in the product reach the Anthropic API
directly from the frontend's server side, configured by `ANTHROPIC_API_KEY` and
`ANTHROPIC_BASE_URL`.

So `bedrock.tf` is a prepared seam, not a live path. Two consequences worth
holding onto:

- A failing AI feature is **not** a Bedrock problem. Look at the Anthropic
  credentials and the BFF route, not at IAM or model access.
- Because `ANTHROPIC_BASE_URL` is configurable, routing through Bedrock later is
  a configuration and credential change at that seam rather than a rewrite —
  which is why the IAM half is worth having applied in advance.

## S3 is reached by DuckDB, not by the SDK

The engine does not use `aws-sdk-s3`. Lake access goes through DuckDB's own S3
support, which requires literal `ER_S3_ACCESS_KEY_ID`, `ER_S3_SECRET_ACCESS_KEY`
and a non-empty scheme-less `ER_S3_ENDPOINT` — there is no credential-chain path
and the endpoint cannot be omitted. An empty secret would attach a lake that
authenticates as nobody, so empty values are rejected rather than defaulted.

The ATTACH options and lake semantics belong to the ducklake-lakehouse skill.
What matters here is the operational consequence: **the engine's S3 identity is
a static key, not Pod Identity**, so it is invisible to the Pod Identity
reasoning that covers every other workload, and it does not expire.

## The one long-lived access key

No IAM users exist for humans. Exactly one long-lived access key exists as a
service identity — the S3 lake key above — and it carries a scheduled
retirement for when the credential-chain work lands.

This said *two* until recently. SES was going to need its own key, because its
SMTP interface does not accept credentials derived from temporary ones; the Rust
mailer calls the SES **API** instead and signs with Pod Identity, so that second
key was never created.

Separately, and more dangerous operationally: the `er-builder` profile is backed
by a never-expiring `AdministratorAccess` static key created for the build loop.
It is a documented, deliberate exception with a scheduled deletion before the
production gate, and it is the one identity that none of the three guardrail
layers constrains. See `mcp-and-guardrails.md`.
