# Product direction

## Purpose

Deliver configurable deduplication through APIs. A customer defines a dataset,
maps their source records, chooses matching and survivorship behavior, submits
data, runs resolution and retrieves usable results. The website and connectors
make that workflow easier to use. Its value must stand on its own for customers
who do not use Salesforce.

Salesforce is the initial commercial integration. The expectation that it offers
a short path to revenue is a go-to-market hypothesis to validate with customers,
not a reason to bake Salesforce objects into the shared contract. AppExchange
distribution has its own work and prerequisites; it is not the release gate for
the generic API.

## Planning rules

- Prioritize a complete external-customer workflow: access, dataset configuration,
  durable ingestion, execution, results and recovery. OpenAPI is its integration
  contract, including operations, payloads, errors, authorization and compatibility.
  Generated schemas and a docs viewer alone do not prove the workflow works.
- Treat mapping, matching, survivorship, job control, delivery receipts, result
  export and lineage as shared capabilities. Put provider authentication,
  extraction and external mutations at the connector boundary.
- Split a ticket that mixes a generic API with a Salesforce feature. A generic
  duplicate check does not depend on a packaged Flow action. Exporting results
  does not depend on writing Salesforce Duplicate Record Sets.
- Reuse existing routes and ingestion/execution machinery before proposing new
  ones. Verify gaps against code and linked work; preserve claims and open PRs.
- Sequence Salesforce as the first commercial integration after the shared
  workflow. Plan SFTP as transport expansion on the same delivery contract, with
  inbound polling and outbound result delivery scoped separately. A hosted SFTP
  endpoint is a different product/operations decision from pulling a customer's
  server. Promote expansion when customer evidence warrants it.
- Promise configurable datasets within supported resolution models. Source
  independence does not prove arbitrary entity schemas work. Track model
  generalization explicitly and reject unsupported mappings clearly.
- Rank product outcomes on the live board with explicit dependencies. Security,
  data correctness and operational blockers can preempt that order. Keep
  implementation estimates on executable work; do not sum epics and children.

## Evidence that decides readiness

A customer using a tenant-scoped key and the published contract can configure a
supported dataset, submit records through JSON and file delivery, observe a run,
and retrieve duplicate groups, golden records and lineage without Salesforce or
operator intervention in the dataset workflow. Retrying an accepted delivery
cannot replace its inputs or duplicate its effects. Invalid mappings, rejected
records and failed runs have documented, recoverable outcomes. Demonstrate this
with an executable integration example and contract tests.

Commercial launch also needs an explicit usage/entitlement policy and operational
readiness. Salesforce writeback has its own preview, authorization, approval,
journal and recovery requirements; a successful core run never authorizes an
external mutation.

This reference records intent, not implementation status or a frozen ticket list.
Read the [Product Roadmap](https://github.com/users/athvin/projects/2) before
prioritizing work. Validate edits to this guidance with `make skills-check` and
`make agent-bridge-check`; validate implementation with its owning skill's gates.
