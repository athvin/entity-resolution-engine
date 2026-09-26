/**
 * Full-stack tier preflight: refuse to run without the gate and a live backend,
 * with instructions instead of a cryptic timeout.
 */
export default async function globalSetup() {
  if (process.env.FRONTEND_E2E !== "1") {
    throw new Error(
      "the full-stack tier is gated: run via `make frontend-e2e` (sets FRONTEND_E2E=1)",
    );
  }
  const base = process.env.ERSERVER_BASE_URL ?? "http://localhost:8000";
  try {
    const response = await fetch(`${base}/healthz`);
    if (!response.ok) throw new Error(`healthz returned ${String(response.status)}`);
  } catch (cause) {
    throw new Error(
      `erserver is not responding at ${base} — run \`make frontend-dev\` and \`make frontend-seed\` first`,
      { cause },
    );
  }
}
