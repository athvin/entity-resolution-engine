// PR comments and cleanup for the static Pages project. No frontend code runs here.
import { appendFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const MARKER = "<!-- marketing-preview -->";
export const PROJECT = "dupezero-marketing-preview";
const ACCOUNT = "9eefbe6bc92d11b2bb4e962494765a4c";
const PAGE_SIZE = 100;
const CLOUDFLARE_PAGE_SIZE = 25;

export function clients(env, fetchImpl = fetch) {
  async function request(base, token, path, options = {}) {
    if (!token)
      throw new Error(`Missing credential for ${new URL(base).hostname}`);
    const response = await fetchImpl(base + path, {
      method: options.method ?? "GET",
      headers: {
        Authorization: `Bearer ${token}`,
        Accept: "application/json",
        "Content-Type": "application/json",
        "User-Agent": "marketing-preview-ci",
      },
      ...(options.body === undefined
        ? {}
        : { body: JSON.stringify(options.body) }),
      signal: AbortSignal.timeout(30_000),
    });
    if (options.missingOk && response.status === 404) return null;
    if (!response.ok)
      throw new Error(
        `${options.method ?? "GET"} ${new URL(base).hostname}${path.split("?")[0]} failed: HTTP ${response.status}`,
      );
    if (response.status === 204) return null;
    const data = await response.json();
    if (data.success === false)
      throw new Error(`Cloudflare rejected ${path.split("?")[0]}`);
    return data;
  }
  return {
    github: (path, options) =>
      request("https://api.github.com", env.GITHUB_TOKEN, path, options),
    cloudflare: (path, options) =>
      request(
        `https://api.cloudflare.com/client/v4/accounts/${ACCOUNT}/pages/projects/${PROJECT}`,
        env.CLOUDFLARE_API_TOKEN,
        path,
        options,
      ),
  };
}

export function context(env) {
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(env.GITHUB_REPOSITORY ?? ""))
    throw new Error("Invalid repository");
  const number = env.PR_NUMBER || null;
  if (
    number !== null &&
    (!/^[1-9][0-9]*$/.test(number) || !Number.isSafeInteger(Number(number)))
  )
    throw new Error("Invalid PR number");
  return { repo: env.GITHUB_REPOSITORY, number, url: env.PREVIEW_URL };
}

export async function getPull(api, ctx) {
  return api.github(`/repos/${ctx.repo}/pulls/${ctx.number}`);
}

async function comment(api, ctx, body, create = true) {
  let existing;
  for (let page = 1; ; page++) {
    const comments = await api.github(
      `/repos/${ctx.repo}/issues/${ctx.number}/comments?per_page=${PAGE_SIZE}&page=${page}`,
    );
    if (!Array.isArray(comments))
      throw new Error("Invalid GitHub comments response");
    existing = comments.find(
      (c) =>
        c.user?.login === "github-actions[bot]" && c.body?.includes(MARKER),
    );
    if (existing || comments.length < PAGE_SIZE) break;
  }
  if (existing) {
    await api.github(`/repos/${ctx.repo}/issues/comments/${existing.id}`, {
      method: "PATCH",
      body: { body },
    });
  } else if (create) {
    await api.github(`/repos/${ctx.repo}/issues/${ctx.number}/comments`, {
      method: "POST",
      body: { body },
    });
  }
}

export async function deletePreviews(api, ctx) {
  const deployments = [];
  // Collect all pages BEFORE deleting: deleting during pagination shifts entries.
  for (let page = 1; ; page++) {
    const data = await api.cloudflare(
      `/deployments?env=preview&per_page=${CLOUDFLARE_PAGE_SIZE}&page=${page}`,
    );
    if (!Array.isArray(data.result))
      throw new Error("Invalid Cloudflare deployments response");
    deployments.push(
      ...data.result.filter(
        (d) =>
          d.environment === "preview" &&
          d.deployment_trigger?.metadata?.branch === `preview-${ctx.number}`,
      ),
    );
    if (data.result_info?.total_pages !== undefined) {
      if (page >= data.result_info.total_pages) break;
    } else if (data.result.length < CLOUDFLARE_PAGE_SIZE) break;
  }
  for (const deployment of deployments) {
    if (!/^[a-zA-Z0-9-]+$/.test(deployment.id))
      throw new Error("Invalid deployment ID");
    // force removes branch aliases too. The selection above never admits production.
    await api.cloudflare(`/deployments/${deployment.id}?force=true`, {
      method: "DELETE",
      missingOk: true,
    });
  }
  return deployments.length;
}

export async function waitForBuilds(
  api,
  ctx,
  pull,
  {
    sleep = (ms) => new Promise((r) => setTimeout(r, ms)),
    now = Date.now,
  } = {},
) {
  const deadline = now() + 30 * 60_000;
  for (;;) {
    let active = false;
    for (let page = 1; ; page++) {
      const data = await api.github(
        `/repos/${ctx.repo}/actions/workflows/marketing-preview.yml/runs?event=pull_request&branch=${encodeURIComponent(pull.head.ref)}&per_page=${PAGE_SIZE}&page=${page}`,
      );
      if (!Array.isArray(data.workflow_runs))
        throw new Error("Invalid workflow runs response");
      active ||= data.workflow_runs.some(
        (run) =>
          run.status !== "completed" &&
          (run.pull_requests?.some((p) => p.number === Number(ctx.number)) ||
            (run.head_sha === pull.head.sha &&
              run.head_repository?.full_name === ctx.repo)),
      );
      if (active || data.workflow_runs.length < PAGE_SIZE) break;
    }
    if (!active) return;
    if (now() >= deadline)
      throw new Error(
        "Preview build still active; retry cleanup after it finishes",
      );
    await sleep(15_000);
  }
}

export async function cleanup(api, ctx, { wait = false, ...timing } = {}) {
  let pull = await getPull(api, ctx);
  if (pull.state !== "closed") return { removed: false, count: 0 };
  if (wait) {
    await waitForBuilds(api, ctx, pull, timing);
    pull = await getPull(api, ctx);
    if (pull.state !== "closed") return { removed: false, count: 0 }; // Reopened while waiting.
  }
  const count = await deletePreviews(api, ctx);
  // Do not overwrite a new live comment if the PR was reopened during deletion.
  if ((await getPull(api, ctx)).state === "closed") {
    await comment(
      api,
      ctx,
      `${MARKER}\nThe marketing preview was removed because this PR is closed or merged.`,
      false,
    );
  }
  return { removed: true, count };
}

export async function publish(api, ctx) {
  if (
    !new RegExp(`^https://[a-z0-9-]+\\.${PROJECT}\\.pages\\.dev/?$`).test(
      ctx.url ?? "",
    )
  )
    throw new Error("Invalid preview URL");
  if (!ctx.number) return { removed: false }; // Main/manual previews have no PR lifecycle.
  if ((await getPull(api, ctx)).state === "closed") return cleanup(api, ctx);
  await comment(
    api,
    ctx,
    `${MARKER}\n[Open the marketing preview](${ctx.url})\n\nThis link updates with new commits. The PR's previews are deleted when it closes or merges.`,
  );
  // Closure can race the comment write. Cleanup handles that race too.
  if ((await getPull(api, ctx)).state === "closed") return cleanup(api, ctx);
  return { removed: false };
}

async function main() {
  const mode = process.argv[2];
  const ctx = context(process.env);
  const api = clients(process.env);
  let result;
  if (mode === "cleanup") {
    if (!ctx.number) throw new Error("Cleanup requires a PR number");
    result = await cleanup(api, ctx, { wait: true });
  } else if (mode === "after") {
    result = ctx.url ? await publish(api, ctx) : await cleanup(api, ctx);
  } else throw new Error("Expected after or cleanup");
  const summary = result.removed
    ? `PR preview removed (${result.count} deployments deleted).`
    : ctx.url
      ? `[Open the marketing preview](${ctx.url})`
      : "PR is open; previews retained.";
  console.log(summary);
  if (process.env.GITHUB_STEP_SUMMARY)
    await appendFile(process.env.GITHUB_STEP_SUMMARY, summary + "\n");
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
