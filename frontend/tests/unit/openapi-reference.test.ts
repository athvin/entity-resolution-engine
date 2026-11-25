import { describe, expect, it } from "vitest";

import publicDoc from "@/lib/api/openapi.public.json";
import {
  curlFor,
  fieldsOf,
  groupOperations,
  operationsOf,
  resourceOf,
  typeLabel,
  type OpenApiDocument,
} from "@/lib/docs/openapi-reference";

/**
 * The /docs/api reference is a rendering of `openapi.public.json` (#867), so
 * these run against the real document: a resource the grouping forgot, or an
 * operation rendered twice, is a page that lies about the contract.
 */
const doc = publicDoc as unknown as OpenApiDocument;
const ops = operationsOf(doc);
const byId = (id: string) => {
  const op = ops.find((o) => o.id === id);
  if (!op) throw new Error(`no operation ${id}`);
  return op;
};

describe("the public API reference", () => {
  it("places every public operation exactly once", () => {
    const declared = Object.values(doc.paths).reduce(
      (n, item) => n + ["get", "post", "put", "patch", "delete"].filter((m) => m in item).length,
      0,
    );
    const grouped = groupOperations(ops).flatMap((g) => g.operations.map((o) => o.id));
    expect(grouped).toHaveLength(declared);
    expect(new Set(grouped).size).toBe(declared);
  });

  it("leaves only the service probes outside the workflow groups", () => {
    const other = groupOperations(ops).find((g) => g.id === "api-other");
    expect((other?.operations ?? []).map((o) => o.path).sort()).toEqual(["/healthz", "/readyz"]);
  });

  it("never documents an operator route publicly", () => {
    expect(ops.filter((o) => o.role === "operator")).toEqual([]);
  });

  it("documents cursors and filters (the #865 query parameters)", () => {
    const reviews = byId("get_orgs_by_org_datasets_by_ds_reviews");
    expect(reviews.queryParams.map((p) => p.name)).toEqual(
      expect.arrayContaining(["cursor", "limit", "status"]),
    );
    expect(reviews.queryParams.every((p) => !p.required)).toBe(true);
    expect(reviews.queryParams.find((p) => p.name === "cursor")?.description).toContain(
      "next_cursor",
    );
  });

  it("resolves a typed success body to its component and fields (#864)", () => {
    const keys = byId("get_orgs_by_org_api_keys");
    expect(keys.role).toBe("admin");
    expect(keys.response.refs).toEqual(["ApiKeyList"]);
    const list = keys.response.fields.find((f) => f.name === "keys");
    expect(list?.type).toBe("array of ApiKey");
    expect(list?.ref).toBe("ApiKey");
    // The wire form has no hash, so neither does the page.
    expect(fieldsOf({ $ref: "#/components/schemas/ApiKey" }, doc).map((f) => f.name)).not.toContain(
      "key_hash",
    );
  });

  it("reads a flattened struct's fields through allOf", () => {
    const detail = byId("get_orgs_by_org_datasets_by_ds_models_by_name_versions_by_version");
    const fields = fieldsOf({ $ref: `#/components/schemas/${detail.response.refs[0] ?? ""}` }, doc);
    expect(fields.map((f) => f.name)).toEqual(
      expect.arrayContaining(["model_json", "weights", "name"]),
    );
  });

  it("writes the first call with variables, never a key", () => {
    const mint = byId("post_orgs_by_org_api_keys");
    expect(curlFor(mint)).toBe(
      [
        'curl -X POST "$ER_API_ORIGIN/v1/orgs/$ER_ORG/api-keys"',
        '  -H "Authorization: Bearer $ER_API_KEY"',
        '  -H "Content-Type: application/json"',
        `  -d '{"role": "<role>"}'`,
      ].join(" \\\n"),
    );
    const probe = ops.find((o) => o.path === "/healthz");
    expect(probe && curlFor(probe)).not.toContain("Authorization");
    const job = byId("get_orgs_by_org_datasets_by_ds_jobs_by_job_id");
    expect(curlFor(job)).toContain("/datasets/$ER_DATASET/jobs/<job_id>");
  });

  it("labels types without colour or guesswork", () => {
    expect(typeLabel({ type: ["string", "null"] }).label).toBe("string | null");
    expect(typeLabel({ type: "array", items: { $ref: "#/components/schemas/Job" } })).toEqual({
      label: "array of Job",
      ref: "Job",
    });
    expect(resourceOf("/v1/orgs/{org}")).toBe("org");
    expect(resourceOf("/v1/orgs/{org}/datasets/{ds}/reviews/{review_id}")).toBe("reviews");
  });
});
