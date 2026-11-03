import { mkdirSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { fetchMockSourceHash, mockSourceHash, nextBundleIsStale } from "../support/stale-servers";

/**
 * The functional tier's staleness guard.
 *
 * These tests exist for the same reason the guard does: it is the kind of
 * check whose failure path nobody sees until the day it matters, and a guard
 * that has only ever been watched passing is not known to work. Each case
 * below plants the condition and requires the detection.
 */

let root: string | null = null;

function tree(): string {
  root = mkdtempSync(path.join(tmpdir(), "stale-servers-"));
  mkdirSync(path.join(root, "tests", "mock-erserver"), { recursive: true });
  mkdirSync(path.join(root, "src"), { recursive: true });
  return root;
}

/** Set a file's mtime to a fixed point, so ordering is asserted not raced. */
function age(file: string, secondsAgo: number): void {
  const at = new Date(Date.now() - secondsAgo * 1000);
  utimesSync(file, at, at);
}

afterEach(() => {
  if (root) rmSync(root, { recursive: true, force: true });
  root = null;
});

describe("the mock's source hash", () => {
  it("changes when any file in the directory changes", () => {
    const dir = tree();
    const server = path.join(dir, "tests", "mock-erserver", "server.mjs");
    writeFileSync(server, "// one\n");
    const before = mockSourceHash(dir);

    writeFileSync(server, "// two\n");
    expect(mockSourceHash(dir)).not.toBe(before);
  });

  it("covers the fixtures beside the server, not just the server", () => {
    const dir = tree();
    writeFileSync(path.join(dir, "tests", "mock-erserver", "server.mjs"), "// one\n");
    const before = mockSourceHash(dir);

    // An edit to a fixture is just as invisible in an adopted process, which
    // is why the hash is over the directory rather than one file.
    writeFileSync(path.join(dir, "tests", "mock-erserver", "arrow.mjs"), "// helper\n");
    expect(mockSourceHash(dir)).not.toBe(before);
  });

  it("is stable across calls when nothing changed", () => {
    const dir = tree();
    writeFileSync(path.join(dir, "tests", "mock-erserver", "server.mjs"), "// one\n");
    expect(mockSourceHash(dir)).toBe(mockSourceHash(dir));
  });
});

describe("fetching the running mock's hash", () => {
  it("returns null when nothing is listening, because that is not staleness", async () => {
    // Port 1 is never a mock. A closed port must read as "the harness will
    // start one", not as a failure — the guard is one-sided on purpose.
    await expect(fetchMockSourceHash("http://127.0.0.1:1")).resolves.toBeNull();
  });
});

describe("the Next bundle check", () => {
  it("reports the newest source file when the build predates it", () => {
    const dir = tree();
    mkdirSync(path.join(dir, ".next"), { recursive: true });
    const build = path.join(dir, ".next", "BUILD_ID");
    writeFileSync(build, "abc");
    age(build, 600);

    const page = path.join(dir, "src", "page.tsx");
    writeFileSync(page, "export default function Page() {}\n");
    age(page, 60);

    expect(nextBundleIsStale(dir)).toEqual({ newest: path.join("src", "page.tsx") });
  });

  it("passes when the build is newer than every source", () => {
    const dir = tree();
    const page = path.join(dir, "src", "page.tsx");
    writeFileSync(page, "export default function Page() {}\n");
    age(page, 600);

    mkdirSync(path.join(dir, ".next"), { recursive: true });
    const build = path.join(dir, ".next", "BUILD_ID");
    writeFileSync(build, "abc");
    age(build, 60);

    expect(nextBundleIsStale(dir)).toBeNull();
  });

  it("treats no build at all as a first run, not as staleness", () => {
    const dir = tree();
    writeFileSync(path.join(dir, "src", "page.tsx"), "x\n");
    expect(nextBundleIsStale(dir)).toBeNull();
  });

  it("ignores files whose extension the bundle does not carry", () => {
    const dir = tree();
    mkdirSync(path.join(dir, ".next"), { recursive: true });
    const build = path.join(dir, ".next", "BUILD_ID");
    writeFileSync(build, "abc");
    age(build, 600);

    // A stray log or editor swap file is newer, and means nothing.
    const noise = path.join(dir, "src", "notes.log");
    writeFileSync(noise, "scratch\n");
    age(noise, 60);

    expect(nextBundleIsStale(dir)).toBeNull();
  });
});
