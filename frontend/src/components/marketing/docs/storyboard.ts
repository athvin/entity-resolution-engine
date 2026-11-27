/**
 * The API storyboard on /docs: the overview's six steps as the calls a client
 * makes, each response handing the next call the value it needs.
 *
 * Illustrative values, real operations. Every method, path and field shown
 * here is held to `openapi.public.json` by `marketing-api-storyboard.test.ts`,
 * so the storyboard cannot show an operation or a field the contract does not
 * have. Ids are UUIDv7 text, shortened; the key is elided and never a
 * plausible secret. Request bodies are compact, as the guide's curl examples
 * send them; responses keep the guide's spacing and are abbreviated.
 *
 * React-free, so the drift test imports it directly.
 */

/** A response value the storyboard abbreviates: drawn as `{…}` or `[…]`. */
export class Elided {
  constructor(readonly kind: "object" | "array") {}
}
const OBJECT = new Elided("object");
const ARRAY = new Elided("array");

export type StoryJson =
  | string
  | number
  | boolean
  | Elided
  | readonly StoryJson[]
  | { readonly [field: string]: StoryJson };

/** The path parameters the storyboard fills, and what it fills them with. */
export type StoryParam = "org" | "ds" | "version" | "job_id" | "entity_id";

export const STORY_VALUES: Record<StoryParam, string> = {
  org: "$ER_ORG",
  ds: "accounts",
  version: "2",
  job_id: "01a219ad…c4b7",
  entity_id: "01a219b1…3b8e",
};

const KEY = "erk_01a219ac…";

export type Exchange = {
  /** Which of the overview's six steps (0-based) the call belongs to. */
  beat: number;
  method: "GET" | "POST";
  /** The operation, exactly as `openapi.public.json` keys it. */
  operation: string;
  query?: string;
  /** The one line sent: a JSON body, or a header or form field. */
  send?: { body: Record<string, StoryJson> } | { line: string };
  /** The response, abbreviated to the fields that tell the story. */
  response: Record<string, StoryJson>;
  /** True when the response has more fields than it shows. */
  more?: boolean;
  /** The field of `response` whose value the next call uses, as a path. */
  carry?: string;
  /** Where this call uses the value the previous one carried. */
  uses?: StoryParam | "authorization";
};

export const EXCHANGES: readonly Exchange[] = [
  {
    beat: 0,
    method: "POST",
    operation: "/v1/orgs/{org}/api-keys",
    send: { body: { role: "admin" } },
    response: { id: "01a219ac…7d3e", key: KEY, role: "admin" },
    carry: "key",
  },
  {
    beat: 1,
    method: "POST",
    operation: "/v1/orgs/{org}/datasets",
    send: { line: `Authorization: Bearer ${KEY}` },
    uses: "authorization",
    response: { name: "accounts", state: "active" },
    carry: "name",
  },
  {
    beat: 2,
    method: "POST",
    operation: "/v1/orgs/{org}/datasets/{ds}/config/versions",
    uses: "ds",
    send: { body: { yaml: "entity_type: account …" } },
    response: { version: 2, state: "draft" },
    more: true,
    carry: "version",
  },
  {
    beat: 3,
    method: "POST",
    operation: "/v1/orgs/{org}/datasets/{ds}/config/versions/{version}/publish",
    uses: "version",
    response: { version: 2, changed_blocks: ARRAY },
    more: true,
  },
  {
    beat: 3,
    method: "POST",
    operation: "/v1/orgs/{org}/datasets/{ds}/imports",
    query: "?source=csv",
    send: { line: "file=@accounts.csv" },
    response: {
      job: { id: STORY_VALUES.job_id, state: "queued" },
      delivery_id: "01a219ad…5e10",
    },
    carry: "job.id",
  },
  {
    beat: 4,
    method: "GET",
    operation: "/v1/orgs/{org}/datasets/{ds}/jobs/{job_id}",
    uses: "job_id",
    response: { id: STORY_VALUES.job_id, state: "succeeded" },
    more: true,
  },
  {
    beat: 4,
    method: "GET",
    operation: "/v1/orgs/{org}/datasets/{ds}/duplicates",
    response: {
      items: [
        { entity_id: STORY_VALUES.entity_id, company_name: "Meridian Supply", member_count: 3 },
      ],
    },
    more: true,
    carry: "items.0.entity_id",
  },
  {
    beat: 5,
    method: "GET",
    operation: "/v1/orgs/{org}/datasets/{ds}/golden-records/{entity_id}",
    uses: "entity_id",
    response: { golden: OBJECT, members: ARRAY, lineage: ARRAY },
    more: true,
  },
  {
    beat: 5,
    method: "POST",
    operation: "/v1/orgs/{org}/datasets/{ds}/assertions",
    send: { body: { kind: "always", a: "salesforce:ACCT-042", b: "api:7731" } },
    response: { action_id: "01a219b3…9d0f", status: "staged" },
    more: true,
  },
];

/** The value at a dotted path (`job.id`, `items.0.entity_id`) of a response. */
export function valueAt(response: Record<string, StoryJson>, path: string): StoryJson | undefined {
  let value: StoryJson | undefined = response;
  for (const step of path.split(".")) {
    if (value === undefined || typeof value !== "object" || value instanceof Elided) return;
    value = Array.isArray(value)
      ? (value as readonly StoryJson[])[Number(step)]
      : (value as { readonly [field: string]: StoryJson })[step];
  }
  return value;
}

/** The value an exchange hands on, as text. */
export function carried(exchange: Exchange): string | undefined {
  if (!exchange.carry) return;
  const value = valueAt(exchange.response, exchange.carry);
  return typeof value === "string" || typeof value === "number" ? String(value) : undefined;
}

/**
 * A request path split for display: the dataset prefix a narrow stage folds
 * to `…`, then the rest, with the parameter this call took from the previous
 * response marked.
 */
export type PathPart = { text: string; fold?: boolean; param?: StoryParam };

export function pathParts(exchange: Exchange): PathPart[] {
  const parts: PathPart[] = [];
  const fold = exchange.operation.startsWith("/v1/orgs/{org}/datasets/{ds}");
  let rest = exchange.operation;
  if (fold) {
    parts.push({ text: "/v1/orgs/$ER_ORG/datasets", fold: true });
    rest = rest.slice("/v1/orgs/{org}/datasets".length);
  }
  for (const piece of rest.split(/(\{[a-z_]+\})/)) {
    if (!piece) continue;
    const name = /^\{([a-z_]+)\}$/.exec(piece)?.[1] as StoryParam | undefined;
    parts.push(name ? { text: STORY_VALUES[name], param: name } : { text: piece });
  }
  if (exchange.query) parts.push({ text: exchange.query });
  return parts;
}
