"use client";

import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { bffFetch, BffRequestError } from "@/lib/api/client";
import { effectiveRole, useSession } from "@/lib/query/hooks";

interface Pair {
  variant_a: string;
  variant_b: string;
}

/**
 * Standalone nickname-lexicon editor (name synonyms used in matching). It is NOT
 * part of the config draft/publish flow — the lexicon is a separate per-org input
 * — so it reads/writes `/api/orgs/{org}/lexicon` directly. A save enqueues a
 * rebuild to apply the change on the next run.
 */
export function LexiconEditor({ org }: { org: string }) {
  const session = useSession();
  const role = effectiveRole(session.data, org);
  const canEdit = role === "steward" || role === "admin";

  const [pairs, setPairs] = useState<Pair[]>([]);
  const [loading, setLoading] = useState(true);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [a, setA] = useState("");
  const [b, setB] = useState("");

  useEffect(() => {
    let live = true;
    bffFetch<{ pairs: Pair[] }>(`/api/orgs/${org}/lexicon`)
      .then((r) => {
        if (live) setPairs(r.pairs);
      })
      .catch(() => {
        if (live) setError("couldn't load the lexicon");
      })
      .finally(() => {
        if (live) setLoading(false);
      });
    return () => {
      live = false;
    };
  }, [org]);

  function addPair() {
    if (!a.trim() || !b.trim()) return;
    setPairs((cur) => [...cur, { variant_a: a.trim(), variant_b: b.trim() }]);
    setA("");
    setB("");
    setSaved(false);
  }

  function removePair(index: number) {
    setPairs((cur) => cur.filter((_, i) => i !== index));
    setSaved(false);
  }

  async function save() {
    setPending(true);
    setError(null);
    setSaved(false);
    try {
      const result = await bffFetch<{ pairs: Pair[]; apply_job: string }>(
        `/api/orgs/${org}/lexicon`,
        {
          method: "PUT",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ pairs }),
        },
      );
      setPairs(result.pairs);
      setSaved(true);
    } catch (cause) {
      setError(cause instanceof BffRequestError ? cause.message : "couldn't save the lexicon");
    } finally {
      setPending(false);
    }
  }

  return (
    <Card data-testid="lexicon-editor">
      <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
        <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
          Nickname lexicon — name synonyms used in matching
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-3 p-4 pt-2 lg:p-6 lg:pt-2">
        {loading ? (
          <p className="text-muted-foreground text-sm">Loading…</p>
        ) : (
          <>
            <div className="flex flex-col gap-1">
              {pairs.map((pair, index) => (
                <div
                  key={pair.variant_a + "-" + pair.variant_b + "-" + String(index)}
                  className="flex items-center gap-2 text-sm"
                  data-testid="lexicon-pair"
                >
                  <span className="flex-1">{pair.variant_a}</span>
                  <span className="text-muted-foreground" aria-hidden>
                    ↔
                  </span>
                  <span className="flex-1">{pair.variant_b}</span>
                  {canEdit && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => {
                        removePair(index);
                      }}
                      data-testid="lexicon-remove"
                    >
                      Remove
                    </Button>
                  )}
                </div>
              ))}
              {pairs.length === 0 && (
                <p className="text-muted-foreground text-sm">No synonym pairs yet.</p>
              )}
            </div>
            {canEdit && (
              <div className="flex items-center gap-2">
                <Input
                  value={a}
                  onChange={(e) => {
                    setA(e.target.value);
                  }}
                  placeholder="bob"
                  aria-label="first name"
                  data-testid="lexicon-a"
                />
                <span className="text-muted-foreground" aria-hidden>
                  ↔
                </span>
                <Input
                  value={b}
                  onChange={(e) => {
                    setB(e.target.value);
                  }}
                  placeholder="robert"
                  aria-label="second name"
                  data-testid="lexicon-b"
                />
                <Button variant="outline" size="sm" onClick={addPair} data-testid="lexicon-add">
                  Add
                </Button>
              </div>
            )}
            {error && (
              <p className="text-destructive text-sm" role="alert" data-testid="lexicon-error">
                {error}
              </p>
            )}
            {saved && (
              <p className="text-muted-foreground text-sm" data-testid="lexicon-saved">
                Saved — a rebuild was queued to apply the new lexicon on the next run.
              </p>
            )}
            {canEdit && (
              <div>
                <Button onClick={() => void save()} disabled={pending} data-testid="lexicon-save">
                  {pending ? "Saving…" : "Save lexicon"}
                </Button>
              </div>
            )}
          </>
        )}
      </CardContent>
    </Card>
  );
}
