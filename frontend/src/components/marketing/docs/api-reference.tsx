import publicDoc from "@/lib/api/openapi.public.json";
import {
  curlFor,
  fieldsOf,
  groupOperations,
  operationsOf,
  type Field,
  type OpenApiDocument,
} from "@/lib/docs/openapi-reference";
import { CodeExample } from "./code-example";

const doc = publicDoc as unknown as OpenApiDocument;
const GROUPS = groupOperations(operationsOf(doc));
const SCHEMA_NAMES = Object.keys(doc.components?.schemas ?? {}).sort();

/** The workflow groups, for the page's table of contents. */
export const API_REFERENCE_GROUPS = GROUPS.map((g) => ({ id: g.id, title: g.title }));

/**
 * The HTTP method, distinguished without hue (#867, owner decision): every stock
 * viewer colour-codes methods, and the design system has no palette. A read is
 * plain; a write is heavier; a delete is boxed, since it is the one that cannot
 * be taken back.
 */
function Method({ method }: { method: string }) {
  const weight =
    method === "GET"
      ? "docs-api-method"
      : method === "DELETE"
        ? "docs-api-method docs-api-method-delete"
        : "docs-api-method docs-api-method-write";
  return <span className={weight}>{method}</span>;
}

function FieldTable({ fields, caption }: { fields: Field[]; caption: string }) {
  if (fields.length === 0) return null;
  return (
    // Scrolls within itself on a phone, so a wide schema never widens the page.
    <div className="docs-api-scroll">
      <table className="docs-api-fields">
        <caption>{caption}</caption>
        <thead>
          <tr>
            <th scope="col">Name</th>
            <th scope="col">Type</th>
            <th scope="col">Required</th>
            <th scope="col">Description</th>
          </tr>
        </thead>
        <tbody>
          {fields.map((f) => (
            <tr key={f.name}>
              <td>
                <code>{f.name}</code>
              </td>
              <td>
                {f.ref ? (
                  <a href={`#schema-${f.ref}`}>
                    <code>{f.type}</code>
                  </a>
                ) : (
                  <code>{f.type}</code>
                )}
              </td>
              <td>{f.required ? "required" : "optional"}</td>
              <td>{f.description}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function ApiReference() {
  return (
    <>
      {GROUPS.map((group) => (
        <section id={group.id} key={group.id}>
          <h2>{group.title}</h2>
          <div className="docs-endpoints">
            {group.operations.map((op) => (
              <details key={op.id} id={op.id} data-testid="api-operation">
                <summary>
                  <Method method={op.method} />
                  <code>{op.path}</code>
                  <span className="docs-role">
                    {op.role === "public" ? "public" : `${op.role}+`}
                  </span>
                </summary>
                <div className="docs-endpoint-body">
                  <h3>{op.summary || `${op.method} ${op.path}`}</h3>
                  <p>
                    {op.role === "public"
                      ? "No credential required."
                      : `Bearer key with at least the ${op.role} role, scoped to the organization in the path.`}
                  </p>
                  <FieldTable fields={op.pathParams} caption="Path parameters" />
                  <FieldTable fields={op.queryParams} caption="Query parameters" />
                  {op.body && (
                    <FieldTable
                      fields={op.body.fields}
                      caption={`Request body${op.body.name ? ` (${op.body.name})` : ""}${op.body.required ? "" : ", optional"}`}
                    />
                  )}
                  <div className="docs-api-response">
                    <p>
                      <strong>Success response:</strong>{" "}
                      {op.response.refs.length > 0
                        ? op.response.refs.map((r, i) => (
                            <span key={r}>
                              {i > 0 && " or "}
                              <a href={`#schema-${r}`}>
                                <code>{r}</code>
                              </a>
                            </span>
                          ))
                        : (op.response.description ?? "a JSON object; see the fields below.")}
                    </p>
                    <FieldTable fields={op.response.fields} caption="Response fields" />
                  </div>
                  <CodeExample label={`${op.method} ${op.path}`}>{curlFor(op)}</CodeExample>
                </div>
              </details>
            ))}
          </div>
        </section>
      ))}
      <section id="api-schemas">
        <h2>Schemas</h2>
        <p>
          Every component the operations above name, generated from the server&apos;s response types
          and entity definitions.
        </p>
        <div className="docs-endpoints">
          {SCHEMA_NAMES.map((name) => (
            <details key={name} id={`schema-${name}`} className="docs-schema">
              <summary>
                <code>{name}</code>
              </summary>
              <div className="docs-endpoint-body">
                <FieldTable
                  fields={fieldsOf({ $ref: `#/components/schemas/${name}` }, doc)}
                  caption={`${name} fields`}
                />
              </div>
            </details>
          ))}
        </div>
      </section>
    </>
  );
}
