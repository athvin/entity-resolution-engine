import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

/**
 * Unmount between tests.
 *
 * @testing-library/react auto-registers this only when vitest runs with
 * `globals: true`, and this project does not — so without it every `render()`
 * accumulates in the same document and `getByText` starts failing with "found
 * multiple elements" in whichever test happens to run second. Registered here
 * rather than per-file so the component tier cannot acquire that trap again.
 */
afterEach(cleanup);
