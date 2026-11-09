import type { Metadata } from "next";
import { CodeExample } from "@/components/marketing/docs/code-example";
import { DeveloperReference } from "@/components/marketing/docs/developer-reference";
import "@/components/marketing/docs/docs.css";

export const dynamic = "force-static";
export const metadata: Metadata = {
  title: "Developer docs",
  description:
    "Organization and dataset concepts, authentication, ingestion, resolution, results, corrections, and the current DupeZero API reference.",
  alternates: { canonical: "/docs" },
  openGraph: { title: "DupeZero developer docs", url: "/docs" },
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

export default function DocsPage() {
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
            Dataset creation currently supports <code>person</code> and <code>account</code> entity
            types, fixed at creation. Organization admins can use auto-provisioning when the
            deployment has a provisioning profile; supplying infrastructure paths and lake settings
            requires operator access.
          </p>
          <CodeExample label="Create a person dataset">{`curl --fail-with-body -X POST \
  "$ER_API_ORIGIN/v1/orgs/$ER_ORG/datasets" \
  -H "Authorization: Bearer $ER_API_KEY" \
  -H 'Content-Type: application/json' \
  --data '{"name":"contacts","display_name":"Contacts","entity_type":"person"}'`}</CodeExample>
          <p>
            Creation may initiate provisioning. Fetch the dataset and check its state before
            submitting work. If auto-provisioning is unavailable, have the operator configure the
            environment; inventing a lake path is not a client-side workaround.
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
            . Create a draft with <code>POST …/config/versions</code> and a JSON body containing a{" "}
            <code>yaml</code> string. The service validates the YAML and hashes the configuration.
            Use the returned version for validation and publication operations in the reference
            below.
          </p>
          <CodeExample label="Prepare a draft configuration request">{`# config.yaml is your deployment-reviewed dataset configuration.
python3 -c 'import json; print(json.dumps({"yaml": open("config.yaml").read()}))' > config-request.json
curl --fail-with-body -X POST \
  "$ER_API_ORIGIN/v1/orgs/$ER_ORG/datasets/$ER_DATASET/config/versions" \
  -H "Authorization: Bearer $ER_API_KEY" \
  -H 'Content-Type: application/json' \
  --data-binary @config-request.json`}</CodeExample>
          <p>
            A saved draft is not the active configuration. Publication checks are part of the
            workflow; validation failures need a configuration correction, not a blind retry.
            Matching decides which records belong together. Survivorship decides which field values
            appear on the resulting golden record. Keep these decisions separate.
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
          <p>
            Missing source or file fields return 422; uploads above the deployment’s configured cap
            return 413; inactive state can return 409; unavailable delivery storage can return 503.
            The JSON record-push surface is a separate operation in the reference and caps a request
            at 10,000 records; larger submissions belong in file deliveries. Consult its declared
            body before using it.
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
                ["409", "Inspect current resource state before resubmitting."],
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
            No general idempotency-key contract is established here. Repeating an upload or job
            submission can create another delivery or job. Persist returned identifiers; if a write
            times out, inspect its outcome before issuing a replacement. Only rely on idempotence
            explicitly documented for an operation.
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
            The current pricing direction is per-run compute, with commercial rates still to be
            established. The web application’s existing usage panel meters AI calls and token units;
            those counts are not a billing ledger. Bring-your-own model keys can also incur provider
            charges outside DupeZero.
          </p>
          <p>
            Record volume alone cannot predict runtime or price: comparisons, candidate generation,
            configuration, and actual execution work matter. Confirm a commercial estimate for your
            workload before treating any usage number as cost.
          </p>
        </section>
        <DeveloperReference />
      </article>
    </div>
  );
}
