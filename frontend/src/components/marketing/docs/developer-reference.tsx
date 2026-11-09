import operations from "./operations.json";
import schema from "@/lib/api/openapi.json";
import { CodeExample } from "./code-example";

const GROUPS = [
  {
    id: "dataset-reference",
    title: "Dataset operations",
    items: operations.filter((o) => o.path.includes("/datasets/") && o.role !== "operator"),
  },
  {
    id: "org-reference",
    title: "Organization operations",
    items: operations.filter(
      (o) =>
        o.path.startsWith("/v1/orgs/") && !o.path.includes("/datasets/") && o.role !== "operator",
    ),
  },
  {
    id: "operator-reference",
    title: "Operator and service operations",
    items: operations.filter((o) => o.role === "operator" || !o.path.startsWith("/v1/orgs/")),
  },
];

export function DeveloperReference() {
  return (
    <>
      <section id="reference">
        <p className="story-eyebrow">Reference</p>
        <h2>Every declared operation.</h2>
        <p>
          This inventory is generated from the control plane’s route manifest and response-envelope
          contract. It describes the current implementation, including operator-only routes. It is
          not a promise that every operation is enabled for your deployment.
        </p>
        <p>
          Request schemas below describe JSON body structure where it has been declared. A missing
          schema does not mean an endpoint accepts an empty body. Query parameters and non-JSON
          payloads are described in the workflow guides for ingestion and result retrieval. Response
          descriptors identify the envelope, not a complete response schema.
        </p>
        <p>
          <strong>Role caveat:</strong> dataset creation is recorded as operator in the manifest.
          The auto-provisioning path accepts an organization admin; supplying{" "}
          <code>config_path</code> requires operator access.
        </p>
      </section>
      {GROUPS.map((group) => (
        <section id={group.id} key={group.id}>
          <h2>{group.title}</h2>
          <p>
            {group.items.length} operations. Expand an entry for its path parameters, declared
            request shape, and response envelope.
          </p>
          <div className="docs-endpoints">
            {group.items.map((op) => (
              <details key={op.id} id={op.id} data-testid="docs-operation">
                <summary>
                  <span className="docs-method">{op.method}</span>
                  <code>{op.path}</code>
                  <span className="docs-role">{op.role}</span>
                </summary>
                <div className="docs-endpoint-body">
                  <h3>{op.summary}</h3>
                  <dl>
                    <div>
                      <dt>Authentication / minimum role</dt>
                      <dd>
                        {op.role === "public"
                          ? "No bearer credential required."
                          : `Bearer credential with ${op.role} authority. Organization routes also enforce organization scope.`}
                      </dd>
                    </div>
                    <div>
                      <dt>Path parameters</dt>
                      <dd>{op.params.length ? op.params.join(", ") : "None"}</dd>
                    </div>
                  </dl>
                  {op.body ? (
                    <CodeExample label={`Request schema · ${op.method} ${op.path}`}>
                      {JSON.stringify(op.body, null, 2)}
                    </CodeExample>
                  ) : (
                    <p>
                      {op.needs_body
                        ? "A request body is required; its schema is not yet declared in the route manifest."
                        : "No JSON body schema is declared. Check the workflow guide for multipart uploads and operation-specific requirements."}
                    </p>
                  )}
                  {op.response ? (
                    <CodeExample label={`Response envelope · ${op.method} ${op.path}`}>
                      {JSON.stringify(op.response, null, 2)}
                    </CodeExample>
                  ) : (
                    <p>
                      A response descriptor is not yet generated for this operation. Do not infer
                      its payload from another endpoint.
                    </p>
                  )}
                  <a href={`#${op.id}`} className="story-text-link">
                    Link to this operation
                  </a>
                </div>
              </details>
            ))}
          </div>
        </section>
      ))}
      <section id="schemas">
        <h2>Schema reference</h2>
        <p>
          The generated OpenAPI {schema.openapi} document currently contains component schemas and{" "}
          <strong>no operation definitions</strong>. It cannot yet generate a complete working
          customer client. The reference above is derived separately from the routed implementation.
        </p>
        <p>
          These schemas describe registered wire types. A response envelope may wrap one or more
          records; it is not interchangeable with the record’s schema.
        </p>
        {Object.entries(schema.components.schemas).map(([name, value]) => (
          <details key={name} className="docs-schema">
            <summary>{name}</summary>
            <CodeExample label={`${name} schema`}>{JSON.stringify(value, null, 2)}</CodeExample>
          </details>
        ))}
      </section>
    </>
  );
}
