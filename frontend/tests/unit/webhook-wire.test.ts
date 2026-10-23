import { describe, expect, it } from "vitest";

import { listFrom } from "@/lib/bff/envelope";
import { queryKeys } from "@/lib/query/keys";
import {
  DELIVERY_STATES,
  canSubscribe,
  deliveryMark,
  deliverySearch,
  formatStamp,
  subscriptionLabel,
} from "@/lib/webhooks/format";

/**
 * The envelope unwrap and the webhook log's presentation rules.
 *
 * `listFrom` exists because er-server has no single list envelope — `{jobs}`,
 * `{deliveries}`, `{events}`, `{items}`, and one bare array on `/webhooks` —
 * and the BFF's browser contract is a bare array everywhere. That mismatch has
 * shipped: the jobs route was `forward<unknown[]>` over `{jobs: […]}`, a type
 * assertion about an HTTP body TypeScript cannot check, so the runs page
 * rendered an empty table against the real backend while every mock-backed tier
 * was green. The cast is now a function, and these are its cases.
 */
describe("list envelope unwrapping", () => {
  it("takes the array at the named key", () => {
    expect(listFrom({ jobs: [1, 2] }, "jobs")).toEqual([1, 2]);
    expect(listFrom({ deliveries: ["a"] }, "deliveries")).toEqual(["a"]);
  });

  it("passes a bare array through", () => {
    // `/v1/orgs/{org}/webhooks` really is a bare array (`webhooks_index`
    // returns `Json(json!(out))`), so tolerating both shapes is what lets one
    // helper serve every route in this directory.
    expect(listFrom([1, 2], "webhooks")).toEqual([1, 2]);
  });

  it("answers an empty array rather than letting a wrong key crash a consumer", () => {
    // The failure mode being prevented: the key is wrong, or the envelope
    // changed. An empty list renders an empty state; `undefined` would throw
    // inside a `.map()` three components away from the route that misread it.
    expect(listFrom({ jobs: [1] }, "items")).toEqual([]);
    expect(listFrom({ jobs: null }, "jobs")).toEqual([]);
    expect(listFrom(null, "jobs")).toEqual([]);
    expect(listFrom("not a list", "jobs")).toEqual([]);
    expect(listFrom({ jobs: { 0: "looks indexable" } }, "jobs")).toEqual([]);
  });
});

describe("delivery state marks", () => {
  it("maps the three recorded states", () => {
    expect(deliveryMark("delivered")).toBe("delivered");
    expect(deliveryMark("failed")).toBe("failed");
    expect(deliveryMark("pending")).toBe("pending");
  });

  it("treats an unrecognised state as not-finished-yet", () => {
    // A state added server-side must render as "still due" rather than crash
    // the log: the safe reading of an unknown outcome is that it has not
    // arrived, not that it has.
    expect(deliveryMark("claimed")).toBe("pending");
    expect(deliveryMark("")).toBe("pending");
  });

  it("offers exactly the states er-server's filter accepts", () => {
    // er-server 422s an unknown `state`, naming these three. A filter offering
    // a fourth would produce a request that cannot succeed.
    expect([...DELIVERY_STATES]).toEqual(["pending", "delivered", "failed"]);
  });
});

describe("delivery filters", () => {
  it("uses the server's parameter names", () => {
    const search = deliverySearch({ webhookId: "wh-1", state: "failed" });
    expect(search.get("webhook_id")).toBe("wh-1");
    expect(search.get("state")).toBe("failed");
    expect(search.has("webhookId")).toBe(false);
  });

  it("omits an empty filter instead of sending a blank value", () => {
    // `state=` is not "any" to er-server — it is an unknown state, and a 422.
    // An absent parameter is what "any" means on the wire.
    expect(deliverySearch({}).toString()).toBe("");
    expect(deliverySearch({ webhookId: "", state: "" }).toString()).toBe("");
    expect(deliverySearch({ state: "pending" }).toString()).toBe("state=pending");
  });
});

describe("timestamps", () => {
  it("renders the server's UTC instant, not the reader's wall clock", () => {
    // Deliberately a slice rather than a locale format: the error text beside a
    // delivery quotes the server's time, and showing the two in different zones
    // is how an operator concludes the log is inconsistent.
    expect(formatStamp("2026-09-24T06:00:01Z")).toBe("2026-09-24 06:00:01");
    expect(formatStamp("2026-09-24T06:00:01.482913Z")).toBe("2026-09-24 06:00:01");
  });

  it("renders absent as an em dash", () => {
    expect(formatStamp(null)).toBe("—");
  });
});

describe("the delivery log's subscription column", () => {
  it("resolves the url when the subscription still exists", () => {
    const urls = new Map([["wh-1", "https://hooks.example.com/er"]]);
    expect(subscriptionLabel(urls, "wh-1")).toBe("https://hooks.example.com/er");
  });

  it("falls back to the id for a removed subscription", () => {
    // The log is history, not a join. A removed subscription's deliveries stay
    // in it, and an empty cell there reads as missing data rather than as
    // "this one is gone".
    expect(subscriptionLabel(new Map(), "wh-gone")).toBe("wh-gone");
  });
});

describe("the subscribe guard", () => {
  it("needs both a url and an event", () => {
    expect(canSubscribe("https://hooks.example.com/er", ["job.failed"])).toBe(true);
    expect(canSubscribe("", ["job.failed"])).toBe(false);
    expect(canSubscribe("https://hooks.example.com/er", [])).toBe(false);
  });

  it("does not count whitespace as a url", () => {
    expect(canSubscribe("   ", ["job.failed"])).toBe(false);
  });

  it("is satisfiable, which is the half that failed silently before", () => {
    // The source wizard stalled because a button's enabled condition could
    // never become true and nothing said why. A guard worth having must be
    // shown to PASS for a legitimate input, not only to reject bad ones.
    expect(canSubscribe("https://hooks.example.com/er", ["entity.merged"])).toBe(true);
  });
});

describe("webhook query keys", () => {
  it("nests the filtered delivery key under the unfiltered one", () => {
    // This is load-bearing, not cosmetic. `useDeleteWebhook` invalidates the
    // PARENT key to clear every filter combination at once — a removed
    // subscription has to disappear from the log's filter list too. TanStack
    // matches by key prefix, so if these two ever stopped sharing one the
    // invalidation would silently stop reaching the filtered caches and the
    // only symptom would be a stale dropdown.
    const parent = queryKeys.webhookDeliveriesAll("acme");
    const filtered = queryKeys.webhookDeliveries("acme", { state: "failed" });
    expect(filtered.slice(0, parent.length)).toEqual([...parent]);
  });

  it("gives each filter combination its own cache entry", () => {
    expect(queryKeys.webhookDeliveries("acme", { state: "failed" })).not.toEqual(
      queryKeys.webhookDeliveries("acme", { state: "pending" }),
    );
    // ...and no filters is its own entry rather than undefined, so the
    // unfiltered page does not share a cache with whatever was asked last.
    expect(queryKeys.webhookDeliveries("acme")).toEqual(["org", "acme", "webhook-deliveries", {}]);
  });

  it("scopes subscriptions and the catalog per org", () => {
    expect(queryKeys.webhooks("acme")).not.toEqual(queryKeys.webhooks("other"));
    expect(queryKeys.webhookEvents("acme")).not.toEqual(queryKeys.webhooks("acme"));
  });
});
