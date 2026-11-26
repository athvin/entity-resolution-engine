"use client";
import { useState } from "react";
import { ArrowDown, ArrowRight, Check } from "lucide-react";
import { Button } from "@/components/ui/button";

import {
  MERIDIAN_GOLDEN,
  MERIDIAN_SOURCES,
  type MeridianSourceKey,
  type MeridianValues,
} from "./scene/meridian";

const SOURCE_LABEL: Record<MeridianSourceKey, string> = {
  salesforce: "Salesforce",
  csv: "CSV upload",
  api: "API",
};

const KEYS: Record<string, keyof MeridianValues> = {
  Name: "name",
  Email: "email",
  Phone: "phone",
  Address: "address",
};

/** Why each value won, in the page's own words for its rules. */
const REASONS: Record<string, string> = {
  Name: "Salesforce is the source you trust most for the company name. The other spellings stay linked to the entity.",
  Email:
    "The API record changed most recently, on 2 November, so its email wins. The other addresses stay in the record's history.",
  Phone:
    "Two of the three records hold the same number once it is cleaned, so the most common value wins.",
  Address:
    "The CSV row has the complete address, suite and all, and its parts are taken together from that one record.",
};

/**
 * Trace any field of Meridian's golden record back to the source values it was
 * chosen from, the rule that chose it, and why. The same golden record the
 * page's scene assembles (meridian.ts), so the two cannot disagree.
 *
 * Revived from the home page's first iteration and recast to Meridian; every
 * claim is one /features/golden-records already makes.
 */
export function ProvenanceExplorer() {
  const [selected, setSelected] = useState(0);
  const field = MERIDIAN_GOLDEN[selected] ?? MERIDIAN_GOLDEN[0];
  const key = KEYS[field.field] ?? "name";
  const held: readonly MeridianSourceKey[] = field.held;
  return (
    <div data-testid="marketing-provenance">
      <div className="mb-6 flex flex-wrap gap-2" role="group" aria-label="Trace a field">
        {MERIDIAN_GOLDEN.map((item, index) => (
          <Button
            key={item.field}
            variant={index === selected ? "secondary" : "ghost"}
            className="min-h-11"
            aria-pressed={index === selected}
            onClick={() => {
              setSelected(index);
            }}
          >
            {item.field}
          </Button>
        ))}
      </div>
      <div className="grid items-center gap-3 sm:grid-cols-[1fr_auto_1fr]">
        <ul className="divide-y border-y" aria-label="Source values">
          {MERIDIAN_SOURCES.map((source) => {
            const kept = held.includes(source.key);
            return (
              <li
                key={source.key}
                className={`border-l-2 px-3 py-3 ${kept ? "bg-muted border-l-foreground" : "border-l-transparent"}`}
              >
                <p className="text-muted-foreground mb-1 flex items-center justify-between text-xs">
                  {source.label}
                  {kept && (
                    <span className="flex items-center gap-1 text-xs">
                      <Check size={12} aria-hidden="true" />
                      Selected
                    </span>
                  )}
                </p>
                <p className="text-sm break-words">{source.raw[key] || "—"}</p>
              </li>
            );
          })}
        </ul>
        <ArrowRight
          className="text-muted-foreground hidden sm:block"
          size={18}
          aria-hidden="true"
        />
        <ArrowDown
          className="text-muted-foreground mx-auto sm:hidden"
          size={18}
          aria-hidden="true"
        />
        <div
          className="border-y py-5"
          aria-live="polite"
          aria-atomic="true"
          data-testid="provenance-result"
        >
          <p className="text-muted-foreground text-xs">Golden record · {field.field}</p>
          <p className="mt-3 text-lg font-medium break-words">{field.value}</p>
          <dl className="mt-6 space-y-3 text-xs">
            <div>
              <dt className="text-muted-foreground">{held.length > 1 ? "Sources" : "Source"}</dt>
              <dd className="mt-1">{held.map((source) => SOURCE_LABEL[source]).join(", ")}</dd>
            </div>
            <div>
              <dt className="text-muted-foreground">Rule</dt>
              <dd className="mt-1">{field.rule}</dd>
            </div>
          </dl>
        </div>
      </div>
      <p className="text-muted-foreground mt-5 min-h-16 text-sm leading-relaxed">
        {REASONS[field.field]}
      </p>
    </div>
  );
}
