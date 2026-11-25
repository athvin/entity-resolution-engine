import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ApiKeysView, type ApiKeysViewProps } from "@/components/settings/api-keys-content";
import type { ApiKeyRow } from "@/lib/query/api-keys";

/**
 * The API-key screen's branches (#866).
 *
 * Every one of them is a statement about a credential: whether the plaintext is
 * on screen, whether a key can still be revoked, whether revoking it would
 * break the app itself. A wrong branch renders a plausible screen that either
 * leaks a key the user thinks is gone or offers to cut the app's own access.
 */

function row(over: Partial<ApiKeyRow> = {}): ApiKeyRow {
  return {
    id: "k1",
    org_id: "o",
    role: "viewer",
    created_at: "2026-11-01T09:00:00Z",
    revoked_at: null,
    updated_at: "2026-11-01T09:00:00Z",
    service: false,
    ...over,
  };
}

function props(over: Partial<ApiKeysViewProps> = {}): ApiKeysViewProps {
  return {
    org: "acme",
    isAdmin: true,
    keys: [row()],
    loading: false,
    created: null,
    copied: false,
    role: "viewer",
    busy: false,
    failure: null,
    onRoleChange: vi.fn(),
    onCreate: vi.fn(),
    onCopy: vi.fn(),
    onRevoke: vi.fn(),
    ...over,
  };
}

describe("ApiKeysView", () => {
  it("tells a non-admin what it cannot do instead of rendering a dead form", () => {
    render(<ApiKeysView {...props({ isAdmin: false })} />);
    expect(screen.getByTestId("api-keys-admin-only")).toBeDefined();
    expect(screen.queryByTestId("api-key-create")).toBeNull();
  });

  it("shows the plaintext only while a just-minted key is held", () => {
    const before = render(<ApiKeysView {...props()} />);
    expect(screen.queryByTestId("api-key-secret")).toBeNull();
    before.unmount();

    const onCopy = vi.fn();
    render(
      <ApiKeysView
        {...props({
          created: { id: "k2", key: "erk_k2_plaintext", role: "steward" },
          onCopy,
        })}
      />,
    );
    expect(screen.getByTestId("api-key-plaintext").textContent).toBe("erk_k2_plaintext");
    expect(screen.getByTestId("api-key-secret").textContent).toContain("Shown once");
    fireEvent.click(screen.getByTestId("api-key-copy"));
    expect(onCopy).toHaveBeenCalledOnce();
  });

  it("never renders a plaintext from the list, which has none to render", () => {
    render(<ApiKeysView {...props({ keys: [row({ id: "k9" })] })} />);
    expect(screen.getByTestId("api-keys-list").textContent).not.toContain("erk_");
  });

  it("offers revoke for an active customer key only", () => {
    const onRevoke = vi.fn();
    render(
      <ApiKeysView
        {...props({
          onRevoke,
          keys: [
            row({ id: "live" }),
            row({ id: "gone", revoked_at: "2026-11-02T09:00:00Z" }),
            row({ id: "app", service: true }),
          ],
        })}
      />,
    );
    fireEvent.click(screen.getByTestId("api-key-revoke-live"));
    expect(onRevoke).toHaveBeenCalledWith("live");
    // Already revoked: nothing left to revoke.
    expect(screen.queryByTestId("api-key-revoke-gone")).toBeNull();
    // The app's own credential: revoking it would sign the app out of the org.
    expect(screen.queryByTestId("api-key-revoke-app")).toBeNull();
    expect(screen.getByTestId("api-key-service").textContent).toContain("Used by this app");
  });

  it("offers every mintable role and never operator", () => {
    render(<ApiKeysView {...props()} />);
    const select = screen.getByTestId<HTMLSelectElement>("api-key-role");
    const options = Array.from(select.options).map((o) => o.value);
    expect(options).toEqual(["viewer", "steward", "admin"]);
  });
});
