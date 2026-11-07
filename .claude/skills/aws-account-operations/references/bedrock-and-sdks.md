# The AWS SDK surface, and Bedrock's two halves

## Contents

- One crate has AWS dependencies, and its feature pins are load-bearing
- `aws sso login` does not work for a locally-run `er-server`
- Bedrock: permission is applied, model access is not
- Nothing in the workspace calls Bedrock today
- Two S3 paths, with two different identities
- The one long-lived access key

## One crate has AWS dependencies, and its feature pins are load-bearing

`er-server` is the **only** crate in the workspace with AWS dependencies. Which
crates those are is the generated table in `footprint.md`, reached from the skill
body — read it there rather than from a list here, because this section carried a
hand-copied three-crate `toml` block that was still describing `aws-sdk-s3` as a
future addition after it had been added.

What matters is the *shape* every one of them must keep:

```toml
{ version = "…", default-features = false, features = ["rt-tokio", "behavior-version-latest"] }
```

`default-features = false` is **not** tidiness, and relaxing it breaks the build
rather than merely adding weight. Two independent reasons, both recorded in
`Cargo.toml` beside the deps:

1. The default feature set pulls `aws-lc-rs`, whose `-sys` crate needs `cmake`
   that the `rust:1-bookworm` build image does not have. `rustls-ring` is the
   deliberate substitute — see the rust-conventions skill for the workspace TLS
   policy this is part of.
2. The defaults add credential providers the server cannot use anyway. It
   authenticates by **EKS Pod Identity**, which is the container-credentials
   provider already in the base chain.

Adding any further AWS crate means repeating that same shape. Use `cargo add`
with `--no-default-features` rather than hand-editing, per the repo convention.

## `aws sso login` does not work for a locally-run `er-server`

This follows directly from reason 2 above: the `sso` and `credentials-process`
providers are excluded, so a developer who has run `aws sso login` still has an
`er-server` that cannot authenticate to SES.

That is intended. Dev wants `ER_MAIL_BACKEND=log`, which is what the absent
credential degrades to. A locally-run server failing to send real email is the
correct behaviour, not a setup problem to debug.

(The excluded providers also require a Rust newer than the floor was at the
time, which was a second reason to leave them out — moot now that
`time`'s RUSTSEC-2026-0009 fix raised the workspace floor anyway. The MSRV
itself belongs to the rust-conventions skill; it is not restated here.)

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

## Two S3 paths, with two different identities

Getting these the wrong way round sends a debugging session to the wrong layer,
so they are worth separating explicitly:

| Path | Reached by | Identity |
|---|---|---|
| **lake** reads and writes | nothing — DuckDB has no S3 path here | none; a lake is a filesystem path |
| **dropped-object ingestion** — the poller that downloads a delivered object into the tenant's `drop_root` | `aws-sdk-s3` | EKS Pod Identity |

**Only the second row runs.** The lake row used to read "the static `ER_S3_*`
key"; that key was retired because nothing consumed it — the Rust engine has no
`httpfs`/`aws` load, no `CREATE SECRET` and no `ER_S3_*` reader, so an `s3://`
lake cannot be opened from Rust at all and the data path in-cluster is EFS at
`/srv/er`. The ATTACH options and that gap's status belong to the
ducklake-lakehouse skill.

The operational consequence is now the simple one: **every S3 identity in this
account is Pod Identity**, with no static-key exception to reason around and
nothing lake-shaped that fails to expire. So an S3 failure *is* an association
question — if it is reaching S3 at all, it is the ingestion poller.

The ingestion path keeps the engine reading the local filesystem, which is why
`delivery_preconditions` / `finish_delivery` stay the single landing path rather
than the engine gaining a second way in.

## The one long-lived access key

No IAM users exist for humans, and **no service identity holds a long-lived key
any more.** This section has counted down from two to zero, which is worth
keeping because each one went for a different reason:

- **SES** was going to need its own key, because its SMTP interface does not
  accept credentials derived from temporary ones. The Rust mailer calls the SES
  **API** instead and signs with Pod Identity, so that key was never created.
- **The S3 lake key** was deleted outright. Not migrated — it had no consumer,
  as the section above explains, and the grants it carried already sat on the
  `er-dev-runner-pod` Pod Identity role.

What remains is **one** key, and it is not a service identity: the `er-builder`
profile is backed by a never-expiring `AdministratorAccess` static key created
for the build loop. Read its posture from `builder.tf` rather than from memory —
it is **kept indefinitely by owner decision**, not scheduled for deletion, and
the earlier claim here of "a scheduled deletion before the production gate" was
wrong. It coexists with prod because explicit `DenyProd*` statements override
its Allow, so it cannot mutate `er-prod-*`; the deliberate hole is that
`iam:CreateAccessKey` is not denied. It is still the one identity none of the
three guardrail layers constrains. See `mcp-and-guardrails.md`.
