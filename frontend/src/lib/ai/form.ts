import type { AiSettingsUpdate } from "@/lib/query/ai-settings";

/** What the AI settings form holds while an admin is editing it. */
export interface AiFormState {
  enabled: boolean;
  /** Empty means "use the deployment default", which the body sends as null. */
  model: string;
  /** Empty means "do not change the stored key". */
  apiKey: string;
  /** The explicit delete-the-stored-key checkbox. */
  clearKey: boolean;
}

/**
 * Turn the form into a PUT body.
 *
 * Extracted from the component because this is where a working key gets deleted
 * by accident. The endpoint reads an absent `api_key` as "leave it alone" and a
 * null one as "delete it", so the mapping from an empty text box to one of
 * those two is the whole decision — and getting it wrong is invisible until
 * someone notices the assistant has quietly fallen back to the deployment key.
 *
 * | typed key | clear box | `api_key` |
 * |---|---|---|
 * | yes | either | the key — typing wins, since it is the more specific act |
 * | no | checked | `null` |
 * | no | unchecked | absent |
 */
export function aiSettingsBody(form: AiFormState): AiSettingsUpdate {
  const model = form.model.trim();
  const apiKey = form.apiKey.trim();
  return {
    enabled: form.enabled,
    model: model === "" ? null : model,
    ...(apiKey !== "" ? { api_key: apiKey } : form.clearKey ? { api_key: null } : {}),
  };
}
