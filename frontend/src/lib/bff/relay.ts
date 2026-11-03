import "server-only";

import { env } from "@/lib/env";

import { upstreamFetch } from "./upstream";

/**
 * The BFF's one path to email: erserver's operator relay (`POST /v1/email`).
 *
 * The platform keeps exactly one SMTP stack, one outbox and one retry policy,
 * so nothing here talks to a mail server. Failure is data, not an exception:
 * every caller already returns something usable without email (an invite link
 * to copy, an enumeration-safe 200), and a relay outage must not turn those
 * into errors.
 */
export async function relayEmail(message: {
  to: string;
  template: string;
  params: Record<string, unknown>;
  org?: string;
}): Promise<boolean> {
  try {
    const response = await upstreamFetch({
      path: "/v1/email",
      method: "POST",
      authorization: `Bearer ${env().ERSERVER_OPERATOR_TOKEN}`,
      json: message,
      // No `actingUser`: the relay is the platform sending on its own behalf,
      // and `message.to` is the recipient rather than an acting person.
    });
    return response.ok;
  } catch {
    return false;
  }
}
