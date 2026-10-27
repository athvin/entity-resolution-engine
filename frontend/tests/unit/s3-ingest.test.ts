import { describe, expect, it } from "vitest";

import { dropPathFor, type S3IngestState } from "@/lib/query/s3-ingest";

const configured = (prefix: string | null): S3IngestState => ({
  bucket: "tenant-drops",
  prefix,
  sources: [],
});

describe("dropPathFor", () => {
  it("builds the exact path a file has to land on", () => {
    // This string is the one thing a tenant has to get right, and getting it
    // wrong is silent: the source comes from the directory, so a file one level
    // up classifies as having no source and is passed over.
    expect(dropPathFor(configured("tenants/acme"), "crm")).toBe(
      "s3://tenant-drops/tenants/acme/crm/",
    );
  });

  it("handles a bucket root with no prefix", () => {
    // A null prefix means the bucket root, which is a legitimate setup. The
    // bug this guards is the doubled slash — `s3://bucket//crm/` is a
    // different key prefix in S3, not a cosmetic difference, so a tenant who
    // copied it would be dropping files somewhere nothing watches.
    expect(dropPathFor(configured(null), "crm")).toBe("s3://tenant-drops/crm/");
    expect(dropPathFor(configured(""), "crm")).toBe("s3://tenant-drops/crm/");
  });

  it("does not double the separator when the prefix carries one", () => {
    expect(dropPathFor(configured("tenants/acme/"), "crm")).toBe(
      "s3://tenant-drops/tenants/acme/crm/",
    );
    expect(dropPathFor(configured("tenants/acme///"), "crm")).toBe(
      "s3://tenant-drops/tenants/acme/crm/",
    );
  });

  it("returns null when no bucket is watched", () => {
    // Not an empty string: the panel branches on this to say "not configured"
    // rather than rendering a path that would silently go nowhere.
    expect(dropPathFor({ bucket: null, prefix: null, sources: [] }, "crm")).toBeNull();
    expect(dropPathFor({ bucket: null, prefix: "tenants/acme", sources: [] }, "crm")).toBeNull();
  });

  it("ends with a slash, so it reads as the directory it is", () => {
    const path = dropPathFor(configured("p"), "crm");
    expect(path?.endsWith("/")).toBe(true);
  });
});
