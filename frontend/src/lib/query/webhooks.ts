"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { deliverySearch, type DeliveryFilters } from "@/lib/webhooks/format";
import { queryKeys } from "./keys";

/**
 * One subscription as the index lists it.
 *
 * Deliberately NOT the whole `Webhook` entity: `webhooks_index` projects four
 * fields and the signing secret is never among them, so typing this as the
 * entity would state a contract the server refuses to fulfil. `events` is
 * `unknown` on the entity (a jsonb column) and `string[]` here, because the
 * create path only ever writes an array of catalogued names.
 */
export interface WebhookRow {
  id: string;
  url: string;
  events: string[];
  active: boolean;
}

/** The create response — the only time the signing `secret` is returned. */
export interface WebhookCreated extends WebhookRow {
  secret: string;
}

/** One catalogued event: what it is called and what it means. */
export interface WebhookEvent {
  event: string;
  description: string;
}

/**
 * One recorded delivery.
 *
 * A subset of `WebhookDelivery`, chosen for what a tenant can act on: which
 * subscription, which event, how it ended, how many attempts it cost, and when
 * the next one is due. `payload` is included because "what was actually sent"
 * is the question the log exists to answer — a retry delivers the same bytes
 * the signature was computed over, so the stored body is the authority.
 */
export interface WebhookDeliveryRow {
  id: string;
  webhook_id: string;
  event_type: string;
  state: string;
  attempt: number;
  max_attempts: number;
  response_code: number | null;
  error: string | null;
  next_attempt_at: string | null;
  delivered_at: string | null;
  created_at: string;
  payload: unknown;
}

export function useWebhooks(org: string) {
  return useQuery({
    queryKey: queryKeys.webhooks(org),
    queryFn: () => bffFetch<WebhookRow[]>(`/api/orgs/${org}/webhooks`),
  });
}

export function useWebhookEvents(org: string) {
  return useQuery({
    queryKey: queryKeys.webhookEvents(org),
    queryFn: () => bffFetch<WebhookEvent[]>(`/api/orgs/${org}/webhooks/events`),
    // The catalog changes only when the product does, so it is worth holding
    // across a page's lifetime rather than refetching per form open.
    staleTime: 5 * 60 * 1000,
  });
}

export function useWebhookDeliveries(org: string, filters: DeliveryFilters = {}) {
  const search = deliverySearch(filters);
  const qs = search.toString();
  return useQuery({
    queryKey: queryKeys.webhookDeliveries(org, Object.fromEntries(search)),
    queryFn: () =>
      bffFetch<WebhookDeliveryRow[]>(`/api/orgs/${org}/webhooks/deliveries${qs ? `?${qs}` : ""}`),
  });
}

export interface WebhookInput {
  url: string;
  events: string[];
  secret?: string;
}

export function useCreateWebhook(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: WebhookInput) =>
      bffFetch<WebhookCreated>(`/api/orgs/${org}/webhooks`, {
        method: "POST",
        body: JSON.stringify(body),
      }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.webhooks(org) });
    },
  });
}

export function useDeleteWebhook(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (webhookId: string) =>
      bffFetch<{ deleted: boolean }>(`/api/orgs/${org}/webhooks/${encodeURIComponent(webhookId)}`, {
        method: "DELETE",
      }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.webhooks(org) });
      // The log keeps a deleted subscription's history, so it has to be
      // refetched too: the subscription name beside each row is resolved from
      // the index, and a stale index renders it as an unknown id.
      void queryClient.invalidateQueries({ queryKey: queryKeys.webhookDeliveriesAll(org) });
    },
  });
}

export type { DeliveryFilters };
