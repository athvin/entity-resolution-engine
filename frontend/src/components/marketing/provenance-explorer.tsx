"use client";
import { useState } from "react";
import { ArrowDown, ArrowRight, Check } from "lucide-react";
import { Button } from "@/components/ui/button";

const FIELDS = [
  {
    label: "Company name",
    value: "Northstar Supply",
    source: "Business API",
    rule: "Preferred source",
    reason:
      "The business API is the preferred source for the trading name. The other spellings stay linked to the entity.",
    inputs: ["Northstar Supply Ltd", "NORTHSTAR SUPPLY", "Northstar Supply"],
    selected: 2,
  },
  {
    label: "Domain",
    value: "northstar.example",
    source: "Database",
    rule: "First non-empty",
    reason:
      "The database supplies the first non-empty domain in the configured source order. Missing values do not replace it.",
    inputs: ["northstar.example", "Not provided", "northstar.example"],
    selected: 0,
  },
  {
    label: "Address",
    value: "18 Mercer Street",
    source: "Spreadsheet",
    rule: "Preferred source",
    reason:
      "The reviewed spreadsheet is the preferred source for the address. The abbreviation is standardized in the result.",
    inputs: ["18 Mercer St.", "18 Mercer Street", "18 Mercer St"],
    selected: 1,
  },
] as const;
const SOURCE_NAMES = ["Database", "Spreadsheet", "Business API"];

export function ProvenanceExplorer() {
  const [selected, setSelected] = useState(0);
  const field = FIELDS[selected] ?? FIELDS[0];
  return (
    <section className="border-t" data-testid="marketing-provenance">
      <div className="mx-auto max-w-5xl px-6 py-20 lg:py-28">
        <div className="grid gap-10 lg:grid-cols-[0.8fr_1.2fr] lg:gap-16">
          <div>
            <p className="text-muted-foreground mb-4 text-xs font-medium tracking-widest uppercase">
              Look inside the result
            </p>
            <h2 className="text-2xl font-medium tracking-tight sm:text-3xl">
              A value is useful.
              <br />
              Knowing where it came from is better.
            </h2>
            <p className="text-muted-foreground mt-4 leading-relaxed">
              Choose a field to trace it from source to resolved record. Different fields can trust
              different sources.
            </p>
            <p className="text-muted-foreground mt-4 text-xs">
              Illustrative sample data and survivorship rules.
            </p>
          </div>
          <div>
            <div className="mb-6 flex flex-wrap gap-2" role="group" aria-label="Trace a field">
              {FIELDS.map((item, index) => (
                <Button
                  key={item.label}
                  variant={index === selected ? "secondary" : "ghost"}
                  className="min-h-11"
                  aria-pressed={index === selected}
                  onClick={() => {
                    setSelected(index);
                  }}
                >
                  {item.label}
                </Button>
              ))}
            </div>
            <div className="grid items-center gap-3 sm:grid-cols-[1fr_auto_1fr]">
              <ul className="divide-y border-y" aria-label="Source values">
                {field.inputs.map((value, index) => (
                  <li
                    key={SOURCE_NAMES[index]}
                    className={`border-l-2 px-3 py-3 ${index === field.selected ? "bg-muted border-l-foreground" : "border-l-transparent"}`}
                  >
                    <p className="text-muted-foreground mb-1 flex items-center justify-between text-xs">
                      {SOURCE_NAMES[index]}
                      {index === field.selected && (
                        <span className="flex items-center gap-1 text-xs">
                          <Check size={12} aria-hidden="true" />
                          Selected
                        </span>
                      )}
                    </p>
                    <p className="text-sm break-words">{value}</p>
                  </li>
                ))}
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
                <p className="text-muted-foreground text-xs">Resolved · {field.label}</p>
                <p className="mt-3 text-lg font-medium break-words">{field.value}</p>
                <dl className="mt-6 space-y-3 text-xs">
                  <div>
                    <dt className="text-muted-foreground">Source</dt>
                    <dd className="mt-1">{field.source}</dd>
                  </div>
                  <div>
                    <dt className="text-muted-foreground">Rule</dt>
                    <dd className="mt-1">{field.rule}</dd>
                  </div>
                </dl>
              </div>
            </div>
            <p className="text-muted-foreground mt-5 min-h-16 text-sm leading-relaxed">
              {field.reason}
            </p>
          </div>
        </div>
      </div>
    </section>
  );
}
