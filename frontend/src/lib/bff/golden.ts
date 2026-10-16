import "server-only";
import { tableFromIPC } from "apache-arrow";

import type { AcctGoldenRecord } from "@/lib/entities/bindings/AcctGoldenRecord";
import type { GoldenRecord } from "@/lib/entities/bindings/GoldenRecord";
import { decodeGoldenTable } from "@/lib/entities/golden-page";

import { forwardStream, type EffectiveOrgAccess, type ForwardOptions } from "./proxy";

/** One golden-records page fetched + decoded server-side (CSV export, assistant
 * tools). Mirrors the browser's `fetchGoldenPage`, but reads the stream fully
 * here rather than passing it to the client. */
export interface ServerGoldenPage {
  items: (GoldenRecord | AcctGoldenRecord)[];
  snapshot: number | null;
  nextCursor: string | null;
}

/** Call the read API's golden-records endpoint and decode its Arrow IPC body
 * through the generated reader (person or account, by `er.relation`). */
export async function forwardGoldenPage(
  access: EffectiveOrgAccess,
  path: string,
  options: ForwardOptions = {},
): Promise<ServerGoldenPage> {
  const stream = await forwardStream(access, path, options);
  const buffer = stream.body ? await new Response(stream.body).arrayBuffer() : new ArrayBuffer(0);
  const table = tableFromIPC(new Uint8Array(buffer));
  return {
    items: decodeGoldenTable(table),
    snapshot: stream.snapshot === null ? null : Number(stream.snapshot),
    nextCursor: stream.nextCursor,
  };
}
