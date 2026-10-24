import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AiSettingsView, type AiSettingsViewProps } from "@/components/settings/ai-content";
import { aiSettingsBody } from "@/lib/ai/form";
import type { AiSettings } from "@/lib/query/ai-settings";

/**
 * What the AI settings screen says, and what the form sends.
 *
 * The render branches here are all statements about credentials — whether a key
 * is stored, whose key a call will use, whether this tenant's state is its own
 * or inherited. A wrong branch renders a plausible screen that misinforms an
 * admin about where their API key is going, which nothing but an assertion
 * would catch.
 *
 * `aiSettingsBody` is tested alongside it because the two halves have to agree:
 * the screen offers a delete-the-key checkbox, and the body is what decides
 * whether checking it deletes anything.
 */

function settings(over: Partial<AiSettings> = {}): AiSettings {
  return {
    enabled: true,
    model: null,
    key_set: false,
    key_set_at: null,
    configured: false,
    updated_by: null,
    updated_at: null,
    deployment_has_key: true,
    ...over,
  };
}

function props(over: Partial<AiSettingsViewProps> = {}): AiSettingsViewProps {
  return {
    settings: settings(),
    form: { enabled: true, model: "", apiKey: "", clearKey: false },
    canEdit: true,
    busy: false,
    saved: false,
    failure: null,
    onEnabledChange: vi.fn(),
    onModelChange: vi.fn(),
    onApiKeyChange: vi.fn(),
    onClearKeyChange: vi.fn(),
    onSave: vi.fn(),
    ...over,
  };
}

describe("AiSettingsView", () => {
  it("says the state is inherited until the tenant has been configured", () => {
    const fresh = render(<AiSettingsView {...props()} />);
    expect(screen.getByTestId("ai-inheritance").textContent).toContain("inherited");
    fresh.unmount();

    render(<AiSettingsView {...props({ settings: settings({ configured: true }) })} />);
    expect(screen.getByTestId("ai-inheritance").textContent).toContain("explicit");
  });

  it("warns when the deployment has no key and the tenant has none either", () => {
    render(<AiSettingsView {...props({ settings: settings({ deployment_has_key: false }) })} />);
    expect(screen.getByTestId("ai-no-deployment-key")).toBeDefined();
  });

  it("does NOT warn when the tenant's own key covers a keyless deployment", () => {
    // The case BYO exists for. Warning here would tell an admin their working
    // configuration is broken.
    render(
      <AiSettingsView
        {...props({ settings: settings({ deployment_has_key: false, key_set: true }) })}
      />,
    );
    expect(screen.queryByTestId("ai-no-deployment-key")).toBeNull();
  });

  it("never renders the key, only whether one is stored and when", () => {
    render(
      <AiSettingsView
        {...props({
          settings: settings({ key_set: true, key_set_at: "2026-10-23T09:00:00Z" }),
        })}
      />,
    );
    const field = screen.getByTestId("ai-key");
    // The input is empty and typed as a password: there is no value to leak
    // because the BFF does not return one.
    expect(field).toHaveValue("");
    expect(field).toHaveAttribute("type", "password");
    expect(field).toHaveAttribute("placeholder", "a key is stored for this tenant");
    expect(screen.getByTestId("ai-key-note").textContent).toContain("never returned");
  });

  it("explains the public-API rule only when there is no stored key to replace", () => {
    // Which endpoint a tenant key talks to is the surprising part of BYO, so it
    // belongs on the screen where someone is about to paste one.
    render(<AiSettingsView {...props()} />);
    expect(screen.getByTestId("ai-key-note").textContent).toContain("public Anthropic API");
  });

  it("offers the delete-key checkbox only when a key is stored", () => {
    const none = render(<AiSettingsView {...props()} />);
    expect(screen.queryByTestId("ai-clear-key")).toBeNull();
    none.unmount();

    render(<AiSettingsView {...props({ settings: settings({ key_set: true }) })} />);
    expect(screen.getByTestId("ai-clear-key")).toBeDefined();
  });

  it("gives a non-admin a readable screen with no usable control", () => {
    render(
      <AiSettingsView
        {...props({ canEdit: false, settings: settings({ key_set: true, configured: true }) })}
      />,
    );
    // Readable: the state is all still on screen, which is what makes this page
    // worth showing a steward at all.
    expect(screen.getByTestId("ai-settings")).toBeDefined();
    expect(screen.getByTestId("ai-read-only").textContent).toContain("admin");
    expect(screen.getByTestId("ai-enabled")).toBeDisabled();
    expect(screen.getByTestId("ai-model")).toBeDisabled();
    expect(screen.getByTestId("ai-key")).toBeDisabled();
    expect(screen.getByTestId("ai-save")).toBeDisabled();
    // Not merely disabled — absent. A delete-the-key control is not something
    // to show someone who cannot use it.
    expect(screen.queryByTestId("ai-clear-key")).toBeNull();
  });

  it("locks the key field while the delete box is checked", () => {
    render(
      <AiSettingsView
        {...props({
          settings: settings({ key_set: true }),
          form: { enabled: true, model: "", apiKey: "", clearKey: true },
        })}
      />,
    );
    expect(screen.getByTestId("ai-key")).toBeDisabled();
  });

  it("says it is verifying, because saving makes a real call", () => {
    // The label is the only warning that Save is not instant: it round-trips to
    // the model API before it writes.
    render(<AiSettingsView {...props({ busy: true })} />);
    expect(screen.getByTestId("ai-save").textContent).toContain("Verifying");
    expect(screen.getByTestId("ai-save")).toBeDisabled();
  });

  it("surfaces a failure as an alert", () => {
    render(
      <AiSettingsView
        {...props({ failure: "this AI configuration does not work: 404 model not found" })}
      />,
    );
    const alert = screen.getByTestId("ai-error");
    expect(alert).toHaveAttribute("role", "alert");
    expect(alert.textContent).toContain("model not found");
  });

  it("confirms a save only once it is no longer in flight", () => {
    const flying = render(<AiSettingsView {...props({ saved: true, busy: true })} />);
    expect(screen.queryByTestId("ai-saved")).toBeNull();
    flying.unmount();

    render(<AiSettingsView {...props({ saved: true })} />);
    expect(screen.getByTestId("ai-saved")).toBeDefined();
  });
});

describe("aiSettingsBody", () => {
  const form = { enabled: true, model: "", apiKey: "", clearKey: false };

  it("omits api_key entirely when nothing was typed and nothing cleared", () => {
    // THE regression this function exists for: an absent key means "leave it
    // alone". Sending null here would delete a tenant's stored key every time
    // an admin changed the model.
    const body = aiSettingsBody(form);
    expect("api_key" in body).toBe(false);
  });

  it("sends null only when the delete box is checked", () => {
    expect(aiSettingsBody({ ...form, clearKey: true }).api_key).toBeNull();
  });

  it("prefers a typed key over the delete box", () => {
    // Typing is the more specific act. The container also empties the field
    // when the box is checked, so this is the belt to that braces.
    expect(aiSettingsBody({ ...form, apiKey: "sk-ant-new", clearKey: true }).api_key).toBe(
      "sk-ant-new",
    );
  });

  it("maps an empty model box to null, so it clears rather than being ignored", () => {
    // Null is "back to the deployment default". Omitting it would make the
    // model impossible to unset through the UI.
    expect(aiSettingsBody(form).model).toBeNull();
  });

  it("trims both fields", () => {
    const body = aiSettingsBody({ ...form, model: "  claude-sonnet-5  ", apiKey: "  sk-ant-x  " });
    expect(body.model).toBe("claude-sonnet-5");
    expect(body.api_key).toBe("sk-ant-x");
  });

  it("treats a whitespace-only key as not typed", () => {
    const body = aiSettingsBody({ ...form, apiKey: "   " });
    expect("api_key" in body).toBe(false);
  });

  it("carries the toggle through unchanged", () => {
    expect(aiSettingsBody({ ...form, enabled: false }).enabled).toBe(false);
  });
});
