/**
 * The public API reference, derived from `openapi.public.json` (#867).
 *
 * Pure functions over the document so the page is a rendering of the contract
 * rather than a second description of it: grouping, field tables and the
 * copyable request are all computed here and unit-tested against the real
 * document. There is deliberately no third-party viewer (owner decision on
 * #867): every stock viewer colour-codes HTTP methods, and the design system
 * forbids a palette.
 */

/** The slice of OpenAPI 3.1 this renderer reads. */
export interface SchemaObject {
  type?: string | string[];
  $ref?: string;
  items?: SchemaObject;
  properties?: Record<string, SchemaObject>;
  required?: string[];
  anyOf?: SchemaObject[];
  oneOf?: SchemaObject[];
  allOf?: SchemaObject[];
  additionalProperties?: SchemaObject | boolean;
  description?: string;
  format?: string;
}

interface ParameterObject {
  name: string;
  in: string;
  required?: boolean;
  description?: string;
  schema?: SchemaObject;
}

interface OperationObject {
  operationId?: string;
  summary?: string | null;
  description?: string | null;
  security?: Record<string, string[]>[] | null;
  parameters?: ParameterObject[];
  requestBody?: { required?: boolean; content?: Record<string, { schema?: SchemaObject }> };
  responses?: Record<string, { content?: Record<string, { schema?: SchemaObject }> }>;
}

export interface OpenApiDocument {
  paths: Record<string, Record<string, OperationObject>>;
  components?: { schemas?: Record<string, SchemaObject> };
}

export interface Field {
  name: string;
  type: string;
  required: boolean;
  description: string;
  /** The component this field's type names, for an in-page link. */
  ref: string | null;
}

export interface Operation {
  id: string;
  method: string;
  path: string;
  summary: string;
  /** The minimum bearer role, or `public` for an unauthenticated route. */
  role: string;
  pathParams: Field[];
  queryParams: Field[];
  /** The request body's fields; null when the route takes no body. */
  body: { name: string | null; fields: Field[]; required: boolean } | null;
  /** The success body's component name(s), or a plain description. */
  response: { refs: string[]; fields: Field[]; description: string | null };
}

export interface Group {
  id: string;
  title: string;
  operations: Operation[];
}

/** Resources in the order a customer meets them, under one heading each. */
const WORKFLOW: { id: string; title: string; resources: string[] }[] = [
  {
    id: "api-organization",
    title: "Organization and keys",
    resources: ["org", "api-keys", "datasets", "audit", "connections", "usage"],
  },
  {
    id: "api-configuration",
    title: "Configuration",
    resources: [
      "config",
      "lexicon",
      "ignore-words",
      "models",
      "schedules",
      "segments",
      "s3-ingest",
    ],
  },
  { id: "api-ingest", title: "Ingest", resources: ["records", "imports", "uploads", "extract"] },
  {
    id: "api-run",
    title: "Run and monitor",
    resources: [
      "jobs",
      "runs",
      "resume",
      "suspend",
      "staged",
      "metrics",
      "insights",
      "quality",
      "data-quality",
      "profile",
      "blocking-estimate",
      "match-preview",
    ],
  },
  {
    id: "api-results",
    title: "Results",
    resources: [
      "golden-records",
      "entities",
      "duplicates",
      "match-scores",
      "merge-plans",
      "resolve",
      "entity-links",
      "exports",
    ],
  },
  {
    id: "api-review",
    title: "Review and correct",
    resources: [
      "reviews",
      "assertions",
      "merges",
      "mass-update",
      "labeling",
      "labels",
      "tags",
      "taggings",
    ],
  },
  {
    id: "api-writeback",
    title: "Salesforce writeback",
    resources: ["writeback", "merge-previews", "upsert-previews"],
  },
  { id: "api-webhooks", title: "Webhooks", resources: ["webhooks"] },
];

const METHODS = ["get", "post", "put", "patch", "delete"] as const;

/** `/v1/orgs/{org}/datasets/{ds}/reviews/{id}` → `reviews`; the org itself → `org`. */
export function resourceOf(path: string): string {
  const dataset = /^\/v1\/orgs\/\{org\}\/datasets\/\{ds\}\/([^/]+)/.exec(path);
  if (dataset?.[1]) return dataset[1];
  const org = /^\/v1\/orgs\/\{org\}\/([^/]+)/.exec(path);
  if (org?.[1]) return org[1];
  if (/^\/v1\/orgs\/\{org\}$/.test(path)) return "org";
  return "service";
}

function refName(ref: string): string {
  return ref.split("/").pop() ?? ref;
}

/** A schema's type as one short line: `string`, `integer | null`, `array of Job`. */
export function typeLabel(schema: SchemaObject | undefined): { label: string; ref: string | null } {
  if (!schema) return { label: "any", ref: null };
  if (schema.$ref) {
    const name = refName(schema.$ref);
    return { label: name, ref: name };
  }
  const union = schema.anyOf ?? schema.oneOf;
  if (union) {
    const parts = union.map((s) => typeLabel(s));
    return {
      label: parts.map((p) => p.label).join(" | "),
      ref: parts.find((p) => p.ref)?.ref ?? null,
    };
  }
  if (schema.allOf?.length === 1) return typeLabel(schema.allOf[0]);
  const types = Array.isArray(schema.type) ? schema.type : schema.type ? [schema.type] : [];
  const base = types.filter((t) => t !== "null");
  const nullable = types.includes("null");
  let label: string;
  let ref: string | null = null;
  if (base[0] === "array") {
    const item = typeLabel(schema.items);
    label = `array of ${item.label}`;
    ref = item.ref;
  } else if (
    base[0] === "object" &&
    schema.additionalProperties &&
    typeof schema.additionalProperties === "object"
  ) {
    const value = typeLabel(schema.additionalProperties);
    label = `map of ${value.label}`;
    ref = value.ref;
  } else {
    label = base.join(" | ") || "any";
  }
  return { label: nullable ? `${label} | null` : label, ref };
}

/** The fields of an object schema, resolving one `$ref` (and `allOf` flatten). */
export function fieldsOf(schema: SchemaObject | undefined, doc: OpenApiDocument): Field[] {
  if (!schema) return [];
  const resolved = schema.$ref ? doc.components?.schemas?.[refName(schema.$ref)] : schema;
  if (!resolved) return [];
  const own = Object.entries(resolved.properties ?? {}).map(([name, prop]) => {
    const type = typeLabel(prop);
    return {
      name,
      type: type.label,
      ref: type.ref,
      required: (resolved.required ?? []).includes(name),
      description: prop.description ?? "",
    };
  });
  // A flattened Rust struct (`#[serde(flatten)]`) arrives as allOf: its
  // members' fields are this object's fields.
  const flattened = (resolved.allOf ?? []).flatMap((member) => fieldsOf(member, doc));
  return [...flattened, ...own];
}

function parameterField(p: ParameterObject): Field {
  const type = typeLabel(p.schema);
  return {
    name: p.name,
    type: type.label,
    ref: type.ref,
    required: p.required === true,
    description: p.description ?? "",
  };
}

function successSchema(op: OperationObject): SchemaObject | undefined {
  const ok = Object.entries(op.responses ?? {}).find(([code]) => code.startsWith("2"));
  return ok?.[1].content?.["application/json"]?.schema;
}

export function operationsOf(doc: OpenApiDocument): Operation[] {
  const out: Operation[] = [];
  for (const [path, item] of Object.entries(doc.paths)) {
    for (const method of METHODS) {
      const op = item[method];
      if (!op) continue;
      const bearer = op.security?.find((s) => "bearerAuth" in s)?.bearerAuth;
      const params = op.parameters ?? [];
      const bodySchema = op.requestBody?.content?.["application/json"]?.schema;
      const success = successSchema(op);
      const refs = success
        ? [success, ...(success.anyOf ?? []), ...(success.oneOf ?? [])]
            .map((s) =>
              s.$ref
                ? refName(s.$ref)
                : s.type === "array" && s.items?.$ref
                  ? refName(s.items.$ref)
                  : null,
            )
            .filter((r): r is string => r !== null)
        : [];
      out.push({
        id: op.operationId ?? `${method}_${path}`,
        method: method.toUpperCase(),
        path,
        summary: op.summary ?? "",
        role: bearer?.[0] ?? "public",
        pathParams: params.filter((p) => p.in === "path").map(parameterField),
        queryParams: params.filter((p) => p.in === "query").map(parameterField),
        body: bodySchema
          ? {
              name: bodySchema.$ref ? refName(bodySchema.$ref) : null,
              fields: fieldsOf(bodySchema, doc),
              required: op.requestBody?.required === true,
            }
          : null,
        response: {
          refs: [...new Set(refs)],
          fields: success && !success.anyOf && !success.oneOf ? fieldsOf(success, doc) : [],
          description:
            success === undefined
              ? "No JSON body."
              : success.type === "array"
                ? `An array of ${typeLabel(success.items).label}.`
                : null,
        },
      });
    }
  }
  return out;
}

/** Operations grouped by workflow stage; anything unplaced lands in "Other". */
export function groupOperations(ops: Operation[]): Group[] {
  const groups: Group[] = WORKFLOW.map((w) => ({ id: w.id, title: w.title, operations: [] }));
  const other: Group = { id: "api-other", title: "Service and other", operations: [] };
  for (const op of ops) {
    const resource = resourceOf(op.path);
    const index = WORKFLOW.findIndex((w) => w.resources.includes(resource));
    (index === -1 ? other : groups[index])?.operations.push(op);
  }
  for (const g of [...groups, other]) {
    g.operations.sort(
      (a, b) => a.path.localeCompare(b.path) || METHOD_RANK(a.method) - METHOD_RANK(b.method),
    );
  }
  return [...groups, other].filter((g) => g.operations.length > 0);
}

function METHOD_RANK(method: string): number {
  return METHODS.indexOf(method.toLowerCase() as (typeof METHODS)[number]);
}

/** A placeholder value of the right JSON shape for a body field. */
function placeholder(field: Field): string {
  if (field.type.startsWith("array")) return "[]";
  if (field.type.startsWith("integer") || field.type.startsWith("number")) return "0";
  if (field.type.startsWith("boolean")) return "false";
  if (field.type.startsWith("string")) return `"<${field.name}>"`;
  return "{}";
}

/**
 * The copyable request for an operation. The key and origin are environment
 * variables (the connect flow sets them), so the page never holds a customer
 * credential — owner decision on #867: no live calls from the docs.
 */
export function curlFor(op: Operation): string {
  const url = op.path
    .replace("{org}", "$ER_ORG")
    .replace("{ds}", "$ER_DATASET")
    .replace(/\{([a-z_]+)\}/g, (_, name: string) => `<${name}>`);
  const lines = [`curl -X ${op.method} "$ER_API_ORIGIN${url}"`];
  if (op.role !== "public") lines.push(`  -H "Authorization: Bearer $ER_API_KEY"`);
  const required = op.body?.fields.filter((f) => f.required) ?? [];
  if (op.body) {
    lines.push(`  -H "Content-Type: application/json"`);
    const body = required.map((f) => `"${f.name}": ${placeholder(f)}`).join(", ");
    lines.push(`  -d '{${body}}'`);
  }
  return lines.join(" \\\n");
}
