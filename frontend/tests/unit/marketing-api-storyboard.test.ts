import { describe, expect, it } from "vitest";

import publicDoc from "@/lib/api/openapi.public.json";
import {
  EXCHANGES,
  Elided,
  STORY_VALUES,
  carried,
  pathParts,
  valueAt,
  type StoryJson,
} from "@/components/marketing/docs/storyboard";
import { MERIDIAN_NAME } from "@/components/marketing/scene/meridian";

/*
 * The /docs walkthrough (#1261) shows real operations with illustrative
 * values. These tests hold it to `openapi.public.json`: a call the contract
 * does not have, or a field it does not declare, fails here instead of
 * teaching an integrator something false.
 */

type Schema = {
  $ref?: string;
  anyOf?: Schema[];
  oneOf?: Schema[];
  allOf?: Schema[];
  properties?: Record<string, Schema>;
  items?: Schema;
};
type Body = { content?: Record<string, { schema?: Schema }> };
type Operation = { requestBody?: Body; responses?: Record<string, Body> };

const doc = publicDoc as unknown as {
  paths: Record<string, Record<string, Operation | undefined> | undefined>;
  components: { schemas: Record<string, Schema | undefined> };
};

const operation = (method: string, path: string) => doc.paths[path]?.[method.toLowerCase()];

/** Every concrete alternative a schema can take: refs followed, unions flattened. */
function branches(schema: Schema | undefined): Schema[] {
  if (!schema) return [];
  if (schema.$ref) return branches(doc.components.schemas[schema.$ref.split("/").at(-1) ?? ""]);
  const union = [...(schema.anyOf ?? []), ...(schema.oneOf ?? []), ...(schema.allOf ?? [])];
  return union.length ? union.flatMap(branches) : [schema];
}

/**
 * The shown fields no branch of the schema declares, as dotted paths. A
 * schema whose every branch declares no properties is free-form, so nothing
 * beneath it can be checked; that is not a miss.
 */
function missingFields(value: StoryJson, schema: Schema, path = ""): string[] {
  if (value instanceof Elided || typeof value !== "object") return [];
  const options = branches(schema);
  if (Array.isArray(value)) {
    const items = options.flatMap((option) => (option.items ? [option.items] : []));
    return (value as readonly StoryJson[]).flatMap((item, index) =>
      missingFields(item, { anyOf: items }, `${path}${String(index)}.`),
    );
  }
  if (options.length > 0 && options.every((option) => !option.properties)) return [];
  return Object.entries(value as { readonly [field: string]: StoryJson }).flatMap(
    ([field, item]) => {
      const found = options.flatMap((option) => {
        const declared = option.properties?.[field];
        return declared ? [declared] : [];
      });
      return found.length === 0
        ? [`${path}${field}`]
        : missingFields(item, { anyOf: found }, `${path}${field}.`);
    },
  );
}

const json = (body: Body | undefined) => body?.content?.["application/json"]?.schema;

describe("the /docs API storyboard", () => {
  it("calls only operations the public contract has", () => {
    for (const exchange of EXCHANGES) {
      expect(
        operation(exchange.method, exchange.operation),
        `${exchange.method} ${exchange.operation}`,
      ).toBeDefined();
    }
  });

  it("shows only fields the contract declares, sent and returned", () => {
    for (const exchange of EXCHANGES) {
      const op = operation(exchange.method, exchange.operation);
      const label = `${exchange.method} ${exchange.operation}`;
      const response = json(op?.responses?.["2XX"]);
      expect(response, `${label} has a JSON response`).toBeDefined();
      if (!response) continue;
      expect(missingFields(exchange.response, response), `${label} response`).toEqual([]);
      if (exchange.send && "body" in exchange.send) {
        const request = json(op?.requestBody);
        expect(request, `${label} takes a JSON body`).toBeDefined();
        if (request)
          expect(missingFields(exchange.send.body, request), `${label} body`).toEqual([]);
      }
    }
  });

  it("hands each call exactly the value the previous response returned", () => {
    let last: string | undefined;
    let handedOn = 0;
    for (const exchange of EXCHANGES) {
      const label = `${exchange.method} ${exchange.operation}`;
      if (exchange.uses) {
        expect(last, `${label} uses a value nothing returned`).toBeDefined();
        if (exchange.uses === "authorization") {
          const line = exchange.send && "line" in exchange.send ? exchange.send.line : "";
          expect(line, label).toContain(`Bearer ${last ?? ""}`);
        } else {
          expect(STORY_VALUES[exchange.uses], label).toBe(last);
          expect(pathParts(exchange).some((part) => part.param === exchange.uses)).toBe(true);
        }
        handedOn += 1;
      }
      if (exchange.carry) {
        expect(valueAt(exchange.response, exchange.carry), `${label} carries`).toBeDefined();
        last = carried(exchange);
      }
    }
    expect(handedOn).toBeGreaterThanOrEqual(5);
  });

  it("fills every path parameter it shows", () => {
    for (const exchange of EXCHANGES) {
      const shown = pathParts(exchange)
        .map((part) => part.text)
        .join("");
      expect(shown, exchange.operation).not.toMatch(/[{}]|undefined/);
    }
  });

  it("walks the overview's six steps in order", () => {
    const beats = EXCHANGES.map((exchange) => exchange.beat);
    expect([...beats].sort((a, b) => a - b)).toEqual(beats);
    expect(new Set(beats)).toEqual(new Set([0, 1, 2, 3, 4, 5]));
  });

  it("finds the company the rest of the site follows", () => {
    const duplicates = EXCHANGES.find((exchange) => exchange.operation.endsWith("/duplicates"));
    expect(duplicates && valueAt(duplicates.response, "items.0.company_name")).toBe(MERIDIAN_NAME);
  });
});
