import {
  REMEDY,
  fetchMockSourceHash,
  mockSourceHash,
  nextBundleIsStale,
} from "../support/stale-servers";

/**
 * Functional tier preflight: refuse to run against a server that is serving
 * code older than the working tree.
 *
 * See `tests/support/stale-servers.ts` for why this exists — in short, a stale
 * adopted server gives confident wrong answers, and the only symptom is
 * disagreeing with CI. Failing with the remedy named costs one line of output;
 * not failing cost four full-tier runs and a wrong conclusion.
 *
 * CI never reuses a server (`reuseExistingServer: !process.env.CI`), so this
 * is a local-only guard and skips there rather than spending the filesystem
 * walk on every job.
 */
export default async function globalSetup(): Promise<void> {
  if (process.env.CI) return;
  // The frontend directory. Playwright loads this file through its CommonJS
  // transform, where `import.meta` is a syntax error, and resolves the
  // config's own paths (`./tests/functional`) against the cwd — so the cwd is
  // the one root both agree on.
  const root = process.cwd();

  const running = await fetchMockSourceHash("http://localhost:8010");
  if (running !== null) {
    const onDisk = mockSourceHash(root);
    if (running !== onDisk) {
      throw new Error(
        `the mock erserver on :8010 is serving different sources than the working tree ` +
          `(it reports ${running.slice(0, 12)}, disk is ${onDisk.slice(0, 12)}). ` +
          `It was adopted rather than started, so your edits are not in it — ${REMEDY}.`,
      );
    }
  }

  const stale = nextBundleIsStale(root);
  if (stale) {
    throw new Error(
      `.next/ was built before ${stale.newest} was last edited, so a \`next start\` ` +
        `adopted on :3100 is serving a bundle without that change — ${REMEDY}.`,
    );
  }
}
