"use client";

import { useState } from "react";
import { CircleCheck, CircleDashed, CircleSlash, Plus, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { BffRequestError } from "@/lib/api/client";
import { effectiveRole, useSession } from "@/lib/query/hooks";
import {
  DELIVERY_STATES,
  canSubscribe,
  deliveryMark,
  formatStamp,
  subscriptionLabel,
} from "@/lib/webhooks/format";
import {
  useCreateWebhook,
  useDeleteWebhook,
  useWebhookDeliveries,
  useWebhookEvents,
  useWebhooks,
  type WebhookCreated,
} from "@/lib/query/webhooks";

const GLYPHS = {
  delivered: CircleCheck,
  failed: CircleSlash,
  pending: CircleDashed,
} as const;

/**
 * A delivery's state as a glyph plus its word.
 *
 * The shape-not-hue choice and the unrecognised-state fallback both live in
 * `deliveryMark`, where they are asserted directly; this is only the rendering.
 */
function StateMark({ state }: { state: string }) {
  const mark = deliveryMark(state);
  const Glyph = GLYPHS[mark];
  return (
    <span className="flex items-center gap-1.5">
      <Glyph className="size-3.5 shrink-0" aria-hidden />
      <span className={mark === "failed" ? "font-medium" : undefined}>{state}</span>
    </span>
  );
}

/**
 * The webhook surface (O3a/O3b/O3c's UI, built in O3c-3): which events this
 * system emits, who is subscribed to what, and what came back from every
 * delivery it has attempted.
 *
 * None of this was reachable before. The catalog, the outbox and the delivery
 * log all shipped headless, so a tenant could not discover which events exist,
 * could not register a subscription without an API call, and — the one that
 * actually cost them — could not see that their endpoint had been returning
 * 500s for a day. Those three gaps are the three cards here.
 */
export function WebhooksContent({ org }: { org: string }) {
  const subscriptions = useWebhooks(org);
  const catalog = useWebhookEvents(org);
  const session = useSession();
  const create = useCreateWebhook(org);
  const remove = useDeleteWebhook(org);
  const isAdmin = effectiveRole(session.data, org) === "admin";

  const [url, setUrl] = useState("");
  const [chosen, setChosen] = useState<string[]>([]);
  const [created, setCreated] = useState<WebhookCreated | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filterWebhook, setFilterWebhook] = useState("");
  const [filterState, setFilterState] = useState("");
  const [openPayload, setOpenPayload] = useState<string | null>(null);

  const deliveries = useWebhookDeliveries(org, {
    webhookId: filterWebhook || undefined,
    state: filterState || undefined,
  });

  function fail(cause: unknown) {
    setError(cause instanceof BffRequestError ? cause.message : "that did not work");
  }

  const rows = subscriptions.data ?? [];
  const events = catalog.data ?? [];
  const log = deliveries.data ?? [];
  // The log keeps a deleted subscription's rows, so a url is not always
  // resolvable — the id is shown when it is not, rather than an empty cell that
  // reads as missing data.
  const urlById = new Map(rows.map((r) => [r.id, r.url]));

  return (
    <div className="flex w-full flex-col gap-4 lg:gap-6" data-testid="webhooks-page">
      <Card data-testid="webhooks-subscriptions">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Subscriptions
          </CardTitle>
          <CardDescription>
            Each delivery is signed with the subscription&apos;s HMAC-SHA256 secret in an{" "}
            <span className="font-mono text-xs">X-ER-Signature</span> header, so a receiver can
            authenticate it. Redirects are refused outright — the URL you register is the only one
            we will POST to.
          </CardDescription>
        </CardHeader>
        <CardContent className="overflow-x-auto p-0">
          {subscriptions.isPending ? (
            <Skeleton className="m-4 h-20" />
          ) : rows.length === 0 ? (
            <p className="text-muted-foreground p-4 text-sm">
              Nothing is subscribed, so nothing is being notified.
            </p>
          ) : (
            <table className="w-full min-w-[36rem] text-sm">
              <thead>
                <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                  <th className="px-4 py-1.5 font-medium">URL</th>
                  <th className="py-1.5 pr-4 font-medium">Events</th>
                  {isAdmin && <th className="py-1.5 pr-4 font-medium" />}
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => (
                  <tr key={row.id} className="border-b last:border-0" data-testid="webhook-row">
                    <td className="max-w-[18rem] truncate px-4 py-2 font-mono text-xs">
                      {row.url}
                    </td>
                    <td className="py-2 pr-4">
                      <span className="flex flex-wrap gap-1">
                        {row.events.map((e) => (
                          <span
                            key={e}
                            className="bg-muted rounded px-1.5 py-0.5 font-mono text-xs"
                          >
                            {e}
                          </span>
                        ))}
                      </span>
                    </td>
                    {isAdmin && (
                      <td className="py-2 pr-4 text-right">
                        <Button
                          variant="ghost"
                          size="sm"
                          data-testid={`webhook-delete-${row.id}`}
                          disabled={remove.isPending}
                          onClick={() => {
                            setError(null);
                            remove.mutateAsync(row.id).catch(fail);
                          }}
                        >
                          <Trash2 /> Remove
                        </Button>
                      </td>
                    )}
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </CardContent>
      </Card>

      {isAdmin && (
        <Card data-testid="webhooks-new">
          <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
            <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
              Subscribe
            </CardTitle>
            <CardDescription>
              The events below are the ones this system actually emits. That list is served by the
              server rather than written here, so a subscription cannot name an event that will
              never arrive.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-4 p-4 pt-2 lg:p-6 lg:pt-2">
            <div className="flex flex-col gap-1">
              <Label htmlFor="webhook-url">Delivery URL</Label>
              <Input
                id="webhook-url"
                data-testid="webhook-url"
                value={url}
                placeholder="https://example.com/hooks/er"
                onChange={(e) => {
                  setUrl(e.target.value);
                }}
              />
            </div>

            <fieldset className="flex flex-col gap-2 border-t pt-4">
              <legend className="sr-only">Events</legend>
              {catalog.isPending ? (
                <Skeleton className="h-24" />
              ) : (
                events.map((e) => (
                  <label key={e.event} className="flex items-start gap-2 text-sm">
                    <input
                      type="checkbox"
                      className="mt-1 size-3.5 shrink-0 accent-current"
                      data-testid={`webhook-event-${e.event}`}
                      checked={chosen.includes(e.event)}
                      onChange={(ev) => {
                        setChosen((prev) =>
                          ev.target.checked
                            ? [...prev, e.event]
                            : prev.filter((x) => x !== e.event),
                        );
                      }}
                    />
                    <span>
                      <span className="font-mono text-xs font-medium">{e.event}</span>
                      <span className="text-muted-foreground block text-xs">{e.description}</span>
                    </span>
                  </label>
                ))
              )}
            </fieldset>

            <div className="flex flex-col items-start gap-1.5">
              <Button
                data-testid="webhook-create"
                disabled={create.isPending || !canSubscribe(url, chosen)}
                onClick={() => {
                  setError(null);
                  setCreated(null);
                  create
                    .mutateAsync({ url: url.trim(), events: chosen })
                    .then((made) => {
                      setCreated(made);
                      setUrl("");
                      setChosen([]);
                    })
                    .catch(fail);
                }}
              >
                <Plus /> {create.isPending ? "Subscribing…" : "Subscribe"}
              </Button>
              <p className="text-muted-foreground text-xs">
                A URL that resolves to a private or link-local address is refused: an unguarded
                webhook would let anyone with admin here aim the control plane at our own network.
              </p>
            </div>

            {created && (
              <div
                className="bg-muted/50 flex flex-col gap-1 rounded-md border p-3"
                data-testid="webhook-secret"
              >
                <p className="text-sm font-medium">Signing secret</p>
                <p className="font-mono text-xs break-all">{created.secret}</p>
                <p className="text-muted-foreground text-xs">
                  Shown once and never listed again, like an API key. Store it now — without it a
                  receiver cannot tell our deliveries from anyone else&apos;s.
                </p>
              </div>
            )}
          </CardContent>
        </Card>
      )}

      <Card data-testid="webhooks-deliveries">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Deliveries
          </CardTitle>
          <CardDescription>
            Every event is recorded before any network call, so a receiver that was briefly
            unreachable gets it late rather than never. Six attempts with backoff spans about ten
            minutes; after that the delivery is given up on and stays here saying so.
          </CardDescription>
        </CardHeader>
        <CardContent className="p-0">
          <div className="flex flex-wrap gap-3 border-b p-4">
            <div className="flex flex-col gap-1">
              <Label htmlFor="delivery-webhook">Subscription</Label>
              <select
                id="delivery-webhook"
                data-testid="delivery-filter-webhook"
                className="border-input bg-background h-9 rounded-md border px-2 text-sm"
                value={filterWebhook}
                onChange={(e) => {
                  setFilterWebhook(e.target.value);
                }}
              >
                <option value="">Any</option>
                {rows.map((r) => (
                  <option key={r.id} value={r.id}>
                    {r.url}
                  </option>
                ))}
              </select>
            </div>
            <div className="flex flex-col gap-1">
              <Label htmlFor="delivery-state">State</Label>
              <select
                id="delivery-state"
                data-testid="delivery-filter-state"
                className="border-input bg-background h-9 rounded-md border px-2 text-sm"
                value={filterState}
                onChange={(e) => {
                  setFilterState(e.target.value);
                }}
              >
                <option value="">Any</option>
                {DELIVERY_STATES.map((s) => (
                  <option key={s} value={s}>
                    {s}
                  </option>
                ))}
              </select>
            </div>
          </div>
          <div className="overflow-x-auto">
            {deliveries.isPending ? (
              <Skeleton className="m-4 h-24" />
            ) : log.length === 0 ? (
              <p className="text-muted-foreground p-4 text-sm">
                No deliveries match. A quiet log with active subscriptions means nothing has
                happened yet, not that something is broken.
              </p>
            ) : (
              <table className="w-full min-w-[48rem] text-sm">
                <thead>
                  <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                    <th className="px-4 py-1.5 font-medium">Event</th>
                    <th className="py-1.5 pr-4 font-medium">To</th>
                    <th className="py-1.5 pr-4 font-medium">State</th>
                    <th className="py-1.5 pr-4 font-medium">Attempts</th>
                    <th className="py-1.5 pr-4 font-medium">Response</th>
                    <th className="py-1.5 pr-4 font-medium">Recorded</th>
                  </tr>
                </thead>
                <tbody>
                  {log.map((d) => (
                    <>
                      <tr
                        key={d.id}
                        className="hover:bg-accent/40 cursor-pointer border-b last:border-0"
                        data-testid="delivery-row"
                        onClick={() => {
                          setOpenPayload(openPayload === d.id ? null : d.id);
                        }}
                      >
                        <td className="px-4 py-2 font-mono text-xs">{d.event_type}</td>
                        <td className="max-w-[14rem] truncate py-2 pr-4 font-mono text-xs">
                          {subscriptionLabel(urlById, d.webhook_id)}
                        </td>
                        <td className="py-2 pr-4">
                          <StateMark state={d.state} />
                        </td>
                        <td className="py-2 pr-4 tabular-nums">
                          {d.attempt} / {d.max_attempts}
                        </td>
                        <td className="py-2 pr-4">
                          <span className="tabular-nums">
                            {d.response_code ?? (d.error === null ? "—" : "no response")}
                          </span>
                          {/* The reason is in the ROW, not behind a click. "Could
                              not see that their endpoint had been returning 500s
                              for a day" is the gap this log closes, and a reason
                              one expand away is a reason nobody reads. */}
                          {d.error !== null && (
                            <span className="text-muted-foreground block text-xs">{d.error}</span>
                          )}
                        </td>
                        <td className="text-muted-foreground py-2 pr-4 font-mono text-xs">
                          {formatStamp(d.created_at)}
                        </td>
                      </tr>
                      {openPayload === d.id && (
                        <tr key={`${d.id}-payload`} className="border-b last:border-0">
                          <td colSpan={6} className="bg-muted/40 px-4 py-3">
                            {d.next_attempt_at !== null && d.state === "pending" && (
                              <p className="text-muted-foreground mb-2 text-xs">
                                Next attempt due {formatStamp(d.next_attempt_at)}
                              </p>
                            )}
                            <p className="text-muted-foreground mb-1 text-xs">
                              The exact body that was sent — a retry delivers these same bytes,
                              because the signature is computed over them.
                            </p>
                            <pre className="overflow-x-auto font-mono text-xs">
                              {JSON.stringify(d.payload, null, 2)}
                            </pre>
                          </td>
                        </tr>
                      )}
                    </>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        </CardContent>
      </Card>

      {error !== null && (
        <p className="text-sm font-medium" data-testid="webhooks-error">
          {error}
        </p>
      )}
    </div>
  );
}
