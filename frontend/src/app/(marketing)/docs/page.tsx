import type { Metadata } from "next";

import { Breadcrumbs, LastUpdated, PAGE_UPDATED, SHARE_IMAGE } from "@/components/marketing/seo";
import { CodeExample } from "@/components/marketing/docs/code-example";
import { DeveloperReference } from "@/components/marketing/docs/developer-reference";
import "@/components/marketing/docs/docs.css";
import { MarketingCut } from "@/components/marketing/cut";

export const dynamic = "force-static";
export const metadata: Metadata = {
  title: "Developer docs",
  description:
    "Organization and dataset concepts, authentication, ingestion, resolution, results, corrections, and the current DupeZero API reference.",
  alternates: { canonical: "/docs" },
  openGraph: { title: "DupeZero developer docs", url: "/docs", images: [SHARE_IMAGE] },
  twitter: {
    card: "summary_large_image",
    title: "DupeZero developer docs",
    images: [SHARE_IMAGE.url],
  },
};
const CONTENTS: [string, string][] = [
  ["overview", "Start here"],
  ["organizations", "Organizations & datasets"],
  ["authentication", "Authentication & roles"],
  ["configuration", "Sources & configuration"],
  ["ingestion", "Submit records"],
  ["jobs", "Run & monitor"],
  ["results", "Retrieve results"],
  ["corrections", "Review & correct"],
  ["integrations", "Integrations"],
  ["errors", "Errors & retries"],
  ["usage", "Usage & pricing"],
  ["reference", "API reference"],
  ["dataset-reference", "Dataset operations"],
  ["org-reference", "Organization operations"],
  ["operator-reference", "Operator operations"],
  ["schemas", "Schemas & contract gaps"],
];
const links = (
  <>
    {CONTENTS.map(([id, title]) => (
      <a href={`#${id}`} key={id}>
        {title}
      </a>
    ))}
  </>
);

/** The page inside its cut: see cut.tsx. */
export default function DocsPage() {
  return (
    <MarketingCut>
      <DocsPageContent />
    </MarketingCut>
  );
}

function DocsPageContent() {
  return (
    <div className="developer-docs" data-testid="developer-docs">
      <aside className="docs-sidebar">
        <p className="story-eyebrow">Developer docs</p>
        <nav aria-label="Documentation contents">{links}</nav>
      </aside>
      <details className="docs-mobile-contents">
        <summary>On this page</summary>
        <nav aria-label="Mobile documentation contents">{links}</nav>
      </details>
      <article className="docs-article">
        <Breadcrumbs path="/docs" label="Docs" />
        <LastUpdated iso={PAGE_UPDATED["/docs"] ?? ""} />
        <section id="overview">
          <p className="story-eyebrow">The details behind the flow</p>
          <h1>Build with DupeZero.</h1>
          <p className="docs-lead">
            Configure a dataset. Submit records. Resolve identities. Retrieve results with their
            evidence.
          </p>
          <p>
            This guide documents the Rust control-plane API as it exists today. The signed-in web
            application calls it through a server-side backend-for-frontend. A browser session
            cookie is not a control-plane API credential.
          </p>
          <p>
            Obtain your deployment’s API origin and an organization-scoped key from your
            administrator. Public production API onboarding and a complete OpenAPI operation
            contract are not yet established by this repository. The examples use environment
            variables for your own configured deployment.
          </p>
          <CodeExample label="Client environment">{`# Use the API origin supplied for your deployment, without a trailing slash.
export ER_API_ORIGIN='https://your-api.example.com'
export ER_ORG='your-org'
export ER_DATASET='contacts'
# Set ER_API_KEY through your secret manager or local shell.
# Never put an operator token in browser code.`}</CodeExample>
          <ol>
            <li>Provision an organization and obtain a scoped key.</li>
            <li>Create or select a dataset and wait for it to become active.</li>
            <li>Configure sources, identity comparisons, review policy, and survivorship.</li>
            <li>Publish a validated configuration, then submit a source delivery.</li>
            <li>Monitor the returned job and retrieve results after success.</li>
            <li>Inspect evidence and apply corrections through reconciliation.</li>
          </ol>
        </section>
        <section id="organizations">
          <h2>One organization. Multiple datasets.</h2>
          <p>
            An organization is the ownership and authorization boundary. It contains datasets, keys,
            connections, and organization-level settings. A dataset is a separate resolution
            workspace with its own source mappings, configuration versions, models, jobs, and
            results. The same upstream system can feed several datasets through their separately
            configured mappings.
          </p>
          <p>
            Paths use the organization and dataset routing names:{" "}
            <code>
              /v1/orgs/{"{org}"}/datasets/{"{ds}"}
            </code>
            . Do not substitute a display name or assume a dataset’s entity identifiers belong to
            another dataset.
          </p>
          <p>
            Dataset creation supports <code>person</code> and <code>account</code> entity types,
            fixed at creation. An organization admin key creates a dataset; the service provisions
            its storage and seeds a starter configuration as version 1. Supplying infrastructure
            paths and lake settings requires operator access.
          </p>
          <CodeExample label="Create a person dataset">{`curl --fail-with-body -X POST \
  "$ER_API_ORIGIN/v1/orgs/$ER_ORG/datasets" \
  -H "Authorization: Bearer $ER_API_KEY" \
  -H 'Content-Type: application/json' \
  --data '{"name":"contacts","display_name":"Contacts","entity_type":"person"}'`}</CodeExample>
          <p>
            Creation returns 202 with a <code>job_id</code>: poll that job, or fetch the dataset
            until its <code>state</code> is <code>active</code>, before publishing configuration or
            submitting work. A dataset still provisioning refuses publication with 409. If
            auto-provisioning is unavailable, have the operator configure the environment; inventing
            a lake path is not a client-side workaround.
          </p>
        </section>
        <section id="authentication">
          <h2>Authentication and roles</h2>
          <p>
            Send <code>Authorization: Bearer &lt;credential&gt;</code> on control-plane requests.
            Organization API keys have the form <code>erk_&lt;key_id&gt;_&lt;secret&gt;</code>. The
            server derives organization scope and role from the stored key. Headers cannot promote a
            caller.
          </p>
          <table>
            <thead>
              <tr>
                <th>Role</th>
                <th>Typical authority</th>
              </tr>
            </thead>
            <tbody>
              <tr>
                <td>viewer</td>
                <td>Read datasets, results, and evidence.</td>
              </tr>
              <tr>
                <td>steward</td>
                <td>Submit deliveries and jobs; record review decisions and corrections.</td>
              </tr>
              <tr>
                <td>admin</td>
                <td>
                  Manage configuration and organization settings; create auto-provisioned datasets.
                </td>
              </tr>
              <tr>
                <td>operator</td>
                <td>Provision infrastructure and operate cross-organization service surfaces.</td>
              </tr>
            </tbody>
          </table>
          <p>
            The hierarchy is viewer → steward → admin → operator. Each endpoint still applies its
            own guard. An invalid or missing credential returns 401; insufficient authority within
            the organization returns 403; an organization outside the key’s scope returns 404.
          </p>
          <CodeExample label="List datasets">{`curl --fail-with-body \
  "$ER_API_ORIGIN/v1/orgs/$ER_ORG/datasets" \
  -H "Authorization: Bearer $ER_API_KEY"`}</CodeExample>
          <p>
            This endpoint returns a bare JSON array, not an <code>items</code> envelope. Keep keys
            in server-side integrations, rotate them through the operator-managed key lifecycle, and
            use the least role your workflow needs.
          </p>
        </section>
        <section id="configuration">
          <h2>Sources, matching, and survivorship</h2>
          <p>
            A published dataset configuration defines the source names, mapped fields,
            standardization, blocking, comparisons, model behavior, and survivorship used by
            resolution. Uploads reference a source name already present in that active
            configuration. Adding a source to the marketing example does not configure a real
            dataset.
          </p>
          <p>
            Read the active configuration at{" "}
            <code>
              GET /v1/orgs/{"{org}"}/datasets/{"{ds}"}/config
            </code>
            . Changing it is three steps, each with an admin key: validate a candidate, store it as
            a draft version, then publish that version. Validation stores nothing and queues
            nothing; it runs every check publication runs and reports all of them at once.
          </p>
          <CodeExample label="Validate a candidate configuration">{`# config.yaml is your dataset configuration.
python3 -c 'import json; print(json.dumps({"yaml": open("config.yaml").read()}))' > config-request.json
curl --fail-with-body -X POST \
  "$ER_API_ORIGIN/v1/orgs/$ER_ORG/datasets/$ER_DATASET/config/validate" \
  -H "Authorization: Bearer $ER_API_KEY" \
  -H 'Content-Type: application/json' \
  --data-binary @config-request.json
# {"valid":false,"problems":["source \`crm\` maps \`emial\`, which is not a canonical person attribute (one of: …)"],
#  "config_hash":"…","tier":"C","changed_blocks":["sources"]}`}</CodeExample>
          <CodeExample label="Store and publish a draft">{`curl --fail-with-body -X POST \
  "$ER_API_ORIGIN/v1/orgs/$ER_ORG/datasets/$ER_DATASET/config/versions" \
  -H "Authorization: Bearer $ER_API_KEY" \
  -H 'Content-Type: application/json' \
  --data-binary @config-request.json          # -> {"version": 2, "state": "draft", ...}
curl --fail-with-body -X POST \
  "$ER_API_ORIGIN/v1/orgs/$ER_ORG/datasets/$ER_DATASET/config/versions/2/publish" \
  -H "Authorization: Bearer $ER_API_KEY"      # -> {"tier": "C", "jobs_enqueued": [...], ...}`}</CodeExample>
          <h3>What a customer configuration may contain</h3>
          <ul>
            <li>
              <strong>Sources.</strong> Each <code>sources.&lt;name&gt;</code> entry maps your own
              column names onto canonical attributes, and names the column carrying the record key (
              <code>record_id_column</code>) and the change timestamp (
              <code>updated_at_column</code>). Person datasets map <code>given_name</code>,{" "}
              <code>family_name</code>, <code>email</code>, <code>phone</code>,{" "}
              <code>address_line</code>, <code>addr_city</code>, <code>addr_region</code>,{" "}
              <code>addr_postal</code> and <code>birth_date</code>; account datasets map{" "}
              <code>company_name</code>, <code>domain</code>, <code>phone</code>,{" "}
              <code>addr_street</code>, <code>addr_city</code>, <code>addr_region</code> and{" "}
              <code>addr_postal</code>. Any other attribute is refused, each of your columns may
              carry one attribute, and every source needs a distinct <code>priority_rank</code>.
            </li>
            <li>
              <strong>Matching.</strong> <code>blocking</code>, <code>constraints</code>,{" "}
              <code>comparisons</code>, <code>comparison_groups</code>, <code>thresholds</code> and{" "}
              <code>models</code> may name only standardized columns of the dataset’s entity type,
              and an ensemble member must have an active model version.
            </li>
            <li>
              <strong>Survivorship.</strong> <code>survivorship</code> and{" "}
              <code>survivorship_groups</code> choose which value reaches the golden record.
            </li>
            <li>
              <strong>Operator-only blocks.</strong> <code>tenant</code>, <code>storage</code>,{" "}
              <code>versions</code>, <code>training</code>, <code>clustering</code>,{" "}
              <code>coherence</code> and <code>generator</code> describe the deployment. An admin
              key may carry them unchanged but not change them; validation reports such a change and
              publication refuses it with 403.
            </li>
          </ul>
          <p>
            The entity type is fixed when the dataset is created, so a configuration declaring the
            other type is refused. Arbitrary entity schemas are not supported: a dataset resolves
            people or accounts through the attributes above.
          </p>
          <h3>What publishing does, and how to roll back</h3>
          <p>
            Publishing classifies what changed against the dataset’s active configuration and queues
            the matching rebuild: a threshold or survivorship change re-clusters (tier A), a
            blocking, constraint or ensemble change re-scores (tier B), and a source,
            standardization or comparison change retrains and rebuilds (tier C, also every first
            publish). Versions are immutable: an edit is a new draft. Every run records the{" "}
            <code>config_hash</code> it executed under, the same value validation and the version
            report, so a later publish never changes what an earlier run meant. To roll back,
            publish an earlier version number again; it is classified and rebuilt like any other
            publish.
          </p>
          <p>
            A saved draft is not the active configuration, and a refused publication needs a
            configuration correction, not a blind retry. Matching decides which records belong
            together. Survivorship decides which field values appear on the resulting golden record.
            Keep these decisions separate.
          </p>
        </section>
        <section id="ingestion">
          <h2>Submit a source delivery</h2>
          <p>
            The file-ingestion endpoint accepts a multipart field named <code>file</code> and a
            required <code>source</code> query parameter. The source must exist in the active
            dataset configuration. The dataset must be active and have a configured delivery
            location.
          </p>
          <CodeExample label="Upload a CSV delivery">{`curl --fail-with-body -X POST \
  "$ER_API_ORIGIN/v1/orgs/$ER_ORG/datasets/$ER_DATASET/imports?source=crm" \
  -H "Authorization: Bearer $ER_API_KEY" \
  -F 'file=@records.csv;type=text/csv'`}</CodeExample>
          <p>
            The server stages an immutable delivery and enqueues its consumer. A successful upload
            returns HTTP 202 with <code>delivery_id</code> and a <code>job</code> object. Record
            both. Accepted means queued, not resolved.
          </p>
          <CodeExample label="Upload response shape (abbreviated)">{`{
  "delivery_id": "<delivery identifier>",
  "job": { "id": "<job identifier>", "state": "queued", "...": "additional job fields" }
}`}</CodeExample>
          <CodeExample label="Push JSON records">{`curl --fail-with-body -X POST \
  "$ER_API_ORIGIN/v1/orgs/$ER_ORG/datasets/$ER_DATASET/records/ingest" \
  -H "Authorization: Bearer $ER_API_KEY" \
  -H 'Content-Type: application/json' \
  -H 'Idempotency-Key: crm-export-2026-11-26' \
  --data '{"source":"crm","records":[{"crm_id":"C1","first_name":"Ada"}]}'`}</CodeExample>
          <h3>Retrying safely</h3>
          <p>
            Send an <code>Idempotency-Key</code> header on every delivery — 1 to 255 visible ASCII
            characters, scoped to the dataset and source. If a request times out, resend it with the
            same key and the same bytes: you receive the original 202 receipt, the same{" "}
            <code>delivery_id</code> and job, with <code>Idempotent-Replayed: true</code>, and no
            second run is queued. Reusing a key for different bytes returns 409 naming the delivery
            that owns it; new data needs a new key. Without a key every request is a new delivery.
          </p>
          <h3>What a delivery changes</h3>
          <p>
            Deliveries <strong>upsert</strong>: each row adds or replaces the record with its source
            record key, and nothing a delivery omits is deleted, so an interrupted or partial upload
            can never remove data. <code>mode=upsert</code> may be stated explicitly;{" "}
            <code>mode=full_refresh</code> is refused with 422 rather than silently ignored. Remove
            records explicitly with <code>POST …/records/delete</code>.
          </p>
          <h3>From receipt to result</h3>
          <table>
            <thead>
              <tr>
                <th>State</th>
                <th>How you see it</th>
              </tr>
            </thead>
            <tbody>
              <tr>
                <td>accepted</td>
                <td>
                  202 with <code>delivery_id</code> and <code>job</code>: the bytes are stored
                  immutably and the run is queued. The run reads exactly these bytes, under the
                  source mapping published when they were accepted.
                </td>
              </tr>
              <tr>
                <td>rejected</td>
                <td>
                  422 before anything is stored: an undeclared column, a missing source, an empty
                  push, a bad key or mode.
                </td>
              </tr>
              <tr>
                <td>queued / running / failed / succeeded</td>
                <td>
                  <code>GET …/jobs/{"{job_id}"}</code>; a failed job carries{" "}
                  <code>error_class</code> and <code>error_detail</code>, and its{" "}
                  <code>run_id</code> once a run started.
                </td>
              </tr>
              <tr>
                <td>result available</td>
                <td>
                  The job succeeded; read duplicates, golden records and lineage. The delivery also
                  appears in <code>GET …/imports</code>.
                </td>
              </tr>
            </tbody>
          </table>
          <p>
            Limits: a JSON push carries at most 10,000 records; a file upload is capped by the
            deployment’s configured import size and returns 413 above it. Files are CSV with a
            header row naming the source’s mapped columns; JSON records are objects keyed by the
            same column names. Other failures: inactive dataset 409, unavailable delivery storage
            503 (retry with backoff and the same key).
          </p>
        </section>
        <section id="jobs">
          <h2>Run and monitor</h2>
          <p>
            An upload already returns the job consuming its delivery. Poll that job rather than
            automatically enqueueing a second full run. For separately requested processing, the job
            endpoint accepts <code>kind</code>, optional <code>priority</code>, and an optional
            configured <code>segment</code> name.
          </p>
          <CodeExample label="Request a resolution run">{`curl --fail-with-body -X POST \
  "$ER_API_ORIGIN/v1/orgs/$ER_ORG/datasets/$ER_DATASET/jobs" \
  -H "Authorization: Bearer $ER_API_KEY" \
  -H 'Content-Type: application/json' \
  --data '{"kind":"run_all_full"}'`}</CodeExample>
          <CodeExample label="Read job status">{`curl --fail-with-body \
  "$ER_API_ORIGIN/v1/orgs/$ER_ORG/datasets/$ER_DATASET/jobs/$ER_JOB_ID" \
  -H "Authorization: Bearer $ER_API_KEY"`}</CodeExample>
          <p>
            Track queued, running, succeeded, failed, and canceled states. A successful submission
            is HTTP 202; the eventual job outcome is separate. Inspect task details for failed work.
            Resume, cancel, and task-clear operations have distinct effects; check their reference
            entries instead of recreating jobs indiscriminately.
          </p>
          <p>
            Use a bounded polling interval with backoff in your client. A segment is a name from
            published configuration, not SQL supplied by the request. Unknown segments are rejected
            at submission.
          </p>
        </section>
        <section id="results">
          <h2>Retrieve results and evidence</h2>
          <p>
            The golden-record list is an <strong>Apache Arrow IPC stream</strong>, not JSON. Read{" "}
            <code>application/vnd.apache.arrow.stream</code> with an Arrow-capable client. The
            response headers carry <code>x-er-snapshot</code> and, when another page exists,{" "}
            <code>x-er-next-cursor</code>.
          </p>
          <CodeExample label="Download a golden-record page">{`curl --fail-with-body --get \
  "$ER_API_ORIGIN/v1/orgs/$ER_ORG/datasets/$ER_DATASET/golden-records" \
  -H "Authorization: Bearer $ER_API_KEY" \
  --data-urlencode 'limit=100' \
  --dump-header result-headers.txt \
  --output records.arrow`}</CodeExample>
          <p>
            For the next page, send the returned opaque value as the <code>cursor</code> query
            parameter. Keep query filters such as <code>q</code> consistent across that traversal.
            The cursor pins the snapshot; do not decode, edit, or construct it. An invalid cursor
            returns 422. Historical snapshots may become unavailable after retention and
            maintenance.
          </p>
          <p>
            <code>GET …/golden-records/{"{entity_id}"}</code> returns JSON detail with members,
            lineage, and events. Its optional <code>snapshot</code> parameter requests a historical
            version. Do not parse a list response as if it were a detail response.
          </p>
          <p>
            Other collections vary: some return bare arrays, some wrap rows under a named key, and
            some return pagination metadata. Each reference entry includes the generated envelope
            where available. There is no universal collection wrapper.
          </p>
        </section>
        <section id="corrections">
          <h2>Review, correct, reconcile</h2>
          <p>
            Use match evidence to understand a proposed grouping before deciding. Review policy
            determines which matches can proceed automatically and which need a person. AI-assisted
            review is a recommendation workflow; it does not remove the need to understand the
            configured approval policy.
          </p>
          <p>
            Assertions record that records must match or must remain separate. Unmerge splits
            selected members out of an entity. These changes may be staged until reconciliation
            applies them; an accepted correction is not the same thing as a refreshed golden record.
          </p>
          <CodeExample label="Request an unmerge">{`curl --fail-with-body -X POST \
  "$ER_API_ORIGIN/v1/orgs/$ER_ORG/datasets/$ER_DATASET/golden-records/$ER_ENTITY_ID/unmerge" \
  -H "Authorization: Bearer $ER_API_KEY" \
  -H 'Content-Type: application/json' \
  --data '{"records":["<member-record-key>"],"apply_now":true}'`}</CodeExample>
          <p>
            Inspect the returned state and any queued job before reading the final result. A
            correction preserves the decision history and changes reconciliation; it does not
            retrain the probability model. Conflicting assertions require resolution, not silent
            precedence. The contradiction inspection operation exposes those conflicts.
          </p>
        </section>
        <section id="integrations">
          <h2>Connect the systems you use</h2>
          <p>
            Source transport, dataset mapping, and destination delivery are separate concerns.
            Salesforce-specific connections and writeback live alongside shared ingestion and result
            contracts. File deliveries, object storage, and custom clients should reuse those shared
            contracts where supported.
          </p>
          <p>
            <strong>ZoomInfo:</strong> the landing-page example shows a proposed custom integration
            on both sides of the flow. This repository does not contain a verified ZoomInfo
            connector. Read access, permitted write operations, field mappings, and account
            entitlements must be established before promising a working read or write path.
          </p>
          <p>
            Webhook registration and delivery operations appear in the reference. Confirm configured
            signing, event selection, and delivery behavior before relying on notifications.
            Reconciliation inside DupeZero does not by itself imply a successful write to an
            external system; track the destination operation independently.
          </p>
        </section>
        <section id="errors">
          <h2>Errors, retries, and compatibility</h2>
          <p>
            Control-plane handler errors generally use a JSON <code>detail</code> field. Request
            extraction and infrastructure failures may differ; always inspect HTTP status and
            content type before decoding a response.
          </p>
          <CodeExample label="Authorization error">{`HTTP/1.1 403 Forbidden
Content-Type: application/json

{"detail":"requires admin role"}`}</CodeExample>
          <table>
            <thead>
              <tr>
                <th>Status</th>
                <th>Client action</th>
              </tr>
            </thead>
            <tbody>
              {[
                ["401", "Check the credential; do not retry the same invalid key."],
                ["403", "Use the required role for this operation."],
                [
                  "404",
                  "Check scope and identifiers; a cross-org request is deliberately indistinguishable from absence.",
                ],
                [
                  "409",
                  "Inspect current resource state before resubmitting; on a delivery, a key reused for different bytes.",
                ],
                ["413", "Reduce the delivery size to the configured cap."],
                ["422", "Correct the request, configuration, or cursor."],
                [
                  "501 / 503",
                  "Check deployment capabilities and service readiness; distinguish missing setup from transient unavailability.",
                ],
                [
                  "5xx / network error",
                  "For safe reads, use bounded backoff. For writes, verify the outcome before retrying.",
                ],
              ].map(([status, action]) => (
                <tr key={status}>
                  <td>{status}</td>
                  <td>{action}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <p>
            Deliveries accept an <code>Idempotency-Key</code> (see{" "}
            <a href="#ingestion">Submit a source delivery</a>), so a timed-out upload or push can be
            resent unchanged. Other writes have no key: repeating a job submission can create
            another job, so persist returned identifiers and inspect the outcome before issuing a
            replacement.
          </p>
          <p>
            The generated OpenAPI schemas do not yet provide complete operation coverage or a
            production compatibility guarantee. Pin your client to the deployed version and test the
            complete workflow against that environment. No live-request console runs from this
            public page.
          </p>
        </section>
        <section id="usage">
          <h2>Usage and pricing</h2>
          <p>
            The Home slider represents records in an example run. It is not a rate card, throughput
            promise, or invoice calculator. Adding sources redistributes that total; it does not
            imply a per-source charge.
          </p>
          <p>
            Pricing is based only on the total records you connect, added together across every
            system, in tiers set out on the pricing page. It does not depend on the number of
            systems, connectors, orgs or users, or on how many runs you do. Records in free sandbox
            or test environments do not count.
          </p>
          <p>
            The web application’s usage panel meters AI calls and token units; those counts are not
            a billing ledger. Bring-your-own model keys can also incur provider charges outside
            DupeZero. Runtime depends on comparisons, candidate generation and configuration, not on
            record volume alone, but it does not change the price.
          </p>
        </section>
        <DeveloperReference />
      </article>
    </div>
  );
}
