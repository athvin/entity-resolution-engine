import test from "node:test";
import assert from "node:assert/strict";
import {
  MARKER,
  clients,
  context,
  cleanup,
  deletePreviews,
  publish,
  waitForBuilds,
} from "./marketing-preview-lifecycle.mjs";

const ctx = {
  repo: "owner/repo",
  number: "42",
  url: "https://abc.dupezero-marketing-preview.pages.dev",
};
const pull = (state) => ({
  state,
  head: { ref: "feature", sha: "a".repeat(40) },
});
const deployment = (id, branch = "preview-42", environment = "preview") => ({
  id,
  environment,
  deployment_trigger: { metadata: { branch } },
});
const existing = {
  id: 7,
  user: { login: "github-actions[bot]" },
  body: MARKER + "\nold link",
};

function fake({
  states = ["open"],
  comments = [existing],
  deployments = [],
} = {}) {
  const calls = [];
  let reads = 0;
  return {
    calls,
    async github(path, options = {}) {
      calls.push({ path, ...options });
      if (path.endsWith("/pulls/42"))
        return pull(states[Math.min(reads++, states.length - 1)]);
      if (path.includes("/comments?")) return comments;
      if (options.method === "PATCH" || options.method === "POST") return {};
      if (path.includes("/runs?")) return { workflow_runs: [] };
      throw new Error("Unexpected GitHub request: " + path);
    },
    async cloudflare(path, options = {}) {
      calls.push({ path, ...options });
      if (options.method === "DELETE") return {};
      return { result: deployments, result_info: { total_pages: 1 } };
    },
  };
}

test("updates the existing bot comment rather than posting another", async () => {
  const api = fake();
  await publish(api, ctx);
  const writes = api.calls.filter((c) => c.method);
  assert.equal(writes.length, 1);
  assert.equal(writes[0].method, "PATCH");
  assert.match(writes[0].body.body, /https:\/\/abc\./);
});

test("does not overwrite a human comment with the marker", async () => {
  const api = fake({ comments: [{ ...existing, user: { login: "human" } }] });
  await publish(api, ctx);
  assert.equal(api.calls.find((c) => c.method).method, "POST");
});

test("finds the bot comment on a later page", async () => {
  const api = fake();
  const original = api.github;
  api.github = async (path, options) =>
    path.endsWith("page=1")
      ? Array.from({ length: 100 }, () => ({ body: "unrelated" }))
      : original(path, options);
  await publish(api, ctx);
  assert.equal(api.calls.find((c) => c.method).method, "PATCH");
});

test("collects all Pages results before deleting only this PR previews", async () => {
  const calls = [];
  const api = {
    cloudflare: async (path, options) => {
      calls.push({ path, ...options });
      if (options) return {};
      return {
        result: path.endsWith("page=1")
          ? [
              deployment("one"),
              deployment("other", "preview-99"),
              deployment("prod", "preview-42", "production"),
              deployment("manual", "preview-100000"),
            ]
          : [deployment("two")],
        result_info: { total_pages: 2 },
      };
    },
  };
  assert.equal(await deletePreviews(api, ctx), 2);
  assert.equal(calls[0].method, undefined);
  assert.equal(calls[1].method, undefined);
  assert.deepEqual(
    calls.slice(2).map((c) => c.path),
    ["/deployments/one?force=true", "/deployments/two?force=true"],
  );
});

test("closed PR deletes previews and marks the comment removed", async () => {
  const api = fake({ states: ["closed"], deployments: [deployment("one")] });
  assert.deepEqual(await publish(api, ctx), { removed: true, count: 1 });
  assert.match(
    api.calls.find((c) => c.method === "PATCH").body.body,
    /removed/,
  );
});

test("closure during comment publication also removes the new deployment", async () => {
  const api = fake({
    states: ["open", "closed"],
    deployments: [deployment("one")],
  });
  assert.equal((await publish(api, ctx)).removed, true);
  const writes = api.calls.filter((c) => c.method === "PATCH");
  assert.equal(writes.length, 2);
  assert.match(writes[1].body.body, /removed/);
});

test("cleanup never deletes an open PR or creates a comment for an unpublished PR", async () => {
  const api = fake();
  assert.equal((await cleanup(api, ctx)).removed, false);
  assert.equal(api.calls.length, 1);
  const closed = fake({ states: ["closed"], comments: [] });
  await cleanup(closed, ctx);
  assert(!closed.calls.some((c) => c.method === "POST"));
});

test("cleanup waits for the in-flight preview workflow before listing deployments", async () => {
  const api = fake({ states: ["closed"] });
  const original = api.github;
  let polls = 0,
    sleeps = 0;
  api.github = async (path, options) =>
    path.includes("/runs?")
      ? {
          workflow_runs:
            ++polls === 1
              ? [{ status: "in_progress", pull_requests: [{ number: 42 }] }]
              : [],
        }
      : original(path, options);
  await cleanup(api, ctx, {
    wait: true,
    sleep: async () => {
      sleeps++;
    },
    now: () => 0,
  });
  assert.equal(sleeps, 1);
  assert.equal(polls, 2);
});

test("reopening while waiting retains previews", async () => {
  const api = fake({ states: ["closed", "open"] });
  assert.equal((await cleanup(api, ctx, { wait: true })).removed, false);
  assert(!api.calls.some((c) => c.path.startsWith("/deployments")));
});

test("a stuck preview run fails cleanup instead of pretending deletion succeeded", async () => {
  const api = {
    github: async () => ({
      workflow_runs: [
        {
          status: "queued",
          head_sha: "a".repeat(40),
          head_repository: { full_name: "owner/repo" },
        },
      ],
    }),
  };
  let time = 0;
  await assert.rejects(
    waitForBuilds(api, ctx, pull("closed"), {
      now: () => time,
      sleep: async () => {
        time += 31 * 60_000;
      },
    }),
    /still active/,
  );
});

test("a failed deletion leaves the existing link comment untouched", async () => {
  const api = fake({ states: ["closed"], deployments: [deployment("one")] });
  const original = api.cloudflare;
  api.cloudflare = (path, options) => {
    if (options?.method === "DELETE") throw new Error("denied");
    return original(path, options);
  };
  await assert.rejects(cleanup(api, ctx), /denied/);
  assert(!api.calls.some((c) => c.method === "PATCH"));
});

test("main/manual previews do not create PR comments or delete deployments", async () => {
  const api = fake();
  await publish(api, { ...ctx, number: null });
  assert.equal(api.calls.length, 0);
});

test("validates PR identifiers and deployment URL origins", async () => {
  for (const number of ["0", "../42", "42;echo", "-1"])
    assert.throws(
      () => context({ GITHUB_REPOSITORY: "owner/repo", PR_NUMBER: number }),
      /Invalid PR/,
    );
  assert.throws(
    () => context({ GITHUB_REPOSITORY: "../outside/repo" }),
    /Invalid repository/,
  );
  await assert.rejects(
    publish(fake(), { ...ctx, url: "https://attacker.example" }),
    /Invalid preview URL/,
  );
});

test("HTTP adapter makes deletion idempotent but fails other API errors without logging secrets", async () => {
  const env = {
    GITHUB_TOKEN: "github-secret",
    CLOUDFLARE_API_TOKEN: "cloudflare-secret",
  };
  const missing = clients(env, async () => new Response("{}", { status: 404 }));
  assert.equal(
    await missing.cloudflare("/deployments/one", {
      method: "DELETE",
      missingOk: true,
    }),
    null,
  );
  const denied = clients(
    env,
    async () => new Response("cloudflare-secret", { status: 403 }),
  );
  await assert.rejects(
    denied.cloudflare("/deployments"),
    (error) =>
      /HTTP 403/.test(error.message) &&
      !error.message.includes("cloudflare-secret"),
  );
  await assert.rejects(
    clients(env, async () => Response.json({ success: false })).cloudflare(
      "/deployments",
    ),
    /Cloudflare rejected/,
  );
});
