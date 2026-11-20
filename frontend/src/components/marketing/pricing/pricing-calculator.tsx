"use client";

import { useId, useMemo, useState } from "react";
import Link from "next/link";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  calculate,
  formatRecords,
  formatUsd,
  GRID_METRIC_LABELS,
  parseRecords,
  RECORDS_ERROR_MESSAGES,
  recordsToSlider,
  referenceGrid,
  sliderToRecords,
  TERM_LABELS,
  TERMS,
  type Calculation,
  type GridMetric,
  type OptionResult,
  type Term,
} from "@/lib/pricing/calculator";
import { PRICING_CONFIG as CONFIG } from "@/lib/pricing/config";

import { SavingsCalculator } from "./savings-calculator";

/**
 * The pricing calculator. Renders what `@/lib/pricing/calculator` returns and
 * does no pricing arithmetic of its own; every figure, rate and URL comes from
 * `@/lib/pricing/config`. Nothing is stored and nothing leaves the page.
 */

const GRID_METRICS = Object.keys(GRID_METRIC_LABELS) as GridMetric[];
const pct = (bp: number) => `${String(bp / 100)}%`;

/** A segmented control: native radios, so arrow keys and labels come for free. */
function Segmented<T extends string>({
  legend,
  name,
  options,
  value,
  onChange,
}: {
  legend: string;
  name: string;
  options: readonly { value: T; label: string }[];
  value: T;
  onChange: (value: T) => void;
}) {
  return (
    <fieldset className="pricing-field">
      <legend className="pricing-label">{legend}</legend>
      <div className="pricing-segmented">
        {options.map((o) => (
          <label key={o.value} data-checked={o.value === value}>
            <input
              type="radio"
              name={name}
              value={o.value}
              checked={o.value === value}
              onChange={() => {
                onChange(o.value);
              }}
            />
            <span>{o.label}</span>
          </label>
        ))}
      </div>
    </fieldset>
  );
}

function OptionFacts({ option }: { option: OptionResult }) {
  const savings = option.term === "monthly" ? "—" : `${String(option.savingsPct)}%`;
  return (
    <dl className="pricing-facts">
      <div>
        <dt>Due at signup</dt>
        <dd>{formatUsd(option.dueAtSignupCents)}</dd>
      </div>
      <div>
        <dt>Cost per year</dt>
        <dd>{formatUsd(option.perYearCents)}</dd>
      </div>
      <div>
        <dt>Cost per month</dt>
        <dd>{formatUsd(option.perMonthCents)}</dd>
      </div>
      <div>
        <dt>{option.term === "monthly" ? "Total for 12 months" : "Total for the term"}</dt>
        <dd>{formatUsd(option.totalCents)}</dd>
      </div>
      <div>
        <dt>Savings vs monthly</dt>
        <dd>{savings}</dd>
      </div>
    </dl>
  );
}

/** What the selected option includes, in words. */
function optionNote(option: OptionResult): string {
  const onboarding =
    option.onboarding === "charged"
      ? `Onboarding ${formatUsd(option.onboardingCents)} once, at signup.`
      : "Onboarding free.";
  const support =
    option.support === "included"
      ? "Support included."
      : option.support === "added"
        ? "Support added."
        : "Support not added.";
  const billing =
    option.billing === "monthly"
      ? "Billed monthly."
      : `Paid upfront for ${String(option.months / 12)} year${option.months > 12 ? "s" : ""}.`;
  return `${billing} ${onboarding} ${support}`;
}

function ContactPanel({ title, body }: { title: string; body: string }) {
  return (
    <div className="pricing-contact" data-testid="pricing-contact">
      <p className="pricing-tier-name">{title}</p>
      <p className="story-muted">{body}</p>
      <Button asChild variant="outline" className="mt-4">
        <a href={CONFIG.SALES_URL}>Talk to sales</a>
      </Button>
    </div>
  );
}

function liveSummary(result: Calculation | null): string {
  if (!result) return "Enter a record count to see pricing.";
  if (result.status === "contact-sales") {
    return `Above ${formatRecords(CONFIG.MAX_ESTIMATE_RECORDS)} records: contact sales.`;
  }
  if (result.status === "contact-us") return "Contact us for pricing at this record count.";
  const o = result.options[result.selected];
  return `${result.tier.name}, ${TERM_LABELS[result.selected]}: ${formatUsd(
    o.perYearCents,
  )} per year, ${formatUsd(o.dueAtSignupCents)} due at signup.`;
}

export function PricingCalculator() {
  const id = useId();
  const [text, setText] = useState(formatRecords(CONFIG.DEFAULT_RECORDS));
  const [records, setRecords] = useState(CONFIG.DEFAULT_RECORDS);
  const [term, setTerm] = useState<Term>("yearly");
  const [support, setSupport] = useState(false);
  const [metric, setMetric] = useState<GridMetric>("perYear");

  const parsed = parseRecords(text);
  const error = parsed.ok ? null : RECORDS_ERROR_MESSAGES[parsed.error];
  const result = useMemo(
    () => (parsed.ok ? calculate({ records, term, support }) : null),
    [parsed.ok, records, term, support],
  );
  const grid = useMemo(() => referenceGrid({ support, metric }), [support, metric]);

  const supportLocked = term === "twoYear" || term === "threeYear";
  const errorId = `${id}-records-error`;
  const hintId = `${id}-records-hint`;
  const helperId = `${id}-records-helper`;

  const onText = (value: string) => {
    setText(value);
    const next = parseRecords(value);
    if (next.ok) setRecords(next.records);
  };

  const priced = result?.status === "priced" ? result : null;
  const maxPerYear = priced ? Math.max(...TERMS.map((t) => priced.options[t].perYearCents)) : 0;

  return (
    <div className="pricing-calculator" data-testid="pricing-calculator">
      <div className="pricing-controls">
        <div className="pricing-field">
          <label className="pricing-label" htmlFor={`${id}-records`}>
            Total records across all connected systems
          </label>
          <p id={helperId} className="pricing-note story-muted">
            Count records from every system you connect, added together.
          </p>
          <Input
            id={`${id}-records`}
            inputMode="numeric"
            autoComplete="off"
            value={text}
            aria-invalid={error !== null}
            aria-describedby={error ? `${errorId} ${helperId} ${hintId}` : `${helperId} ${hintId}`}
            onChange={(e) => {
              onText(e.target.value);
            }}
            onBlur={() => {
              if (parsed.ok) setText(formatRecords(parsed.records));
            }}
            className="pricing-records-input tabular-nums"
            data-testid="pricing-records"
          />
          {error && (
            <p id={errorId} className="pricing-error" data-testid="pricing-records-error">
              {error}
            </p>
          )}
          <input
            type="range"
            className="pricing-slider"
            min={0}
            max={CONFIG.SLIDER_STEPS}
            step={1}
            value={recordsToSlider(records)}
            aria-label="Records, on a logarithmic scale"
            aria-valuetext={`${formatRecords(records)} records`}
            onChange={(e) => {
              const next = sliderToRecords(Number(e.target.value));
              setRecords(next);
              setText(formatRecords(next));
            }}
          />
          <p id={hintId} className="pricing-hint story-muted">
            <span>{formatRecords(CONFIG.SLIDER_MIN_RECORDS)}</span>
            <span>Total across every connected system</span>
            <span>{formatRecords(CONFIG.SLIDER_MAX_RECORDS)}</span>
          </p>
        </div>

        <Segmented
          legend="Billing term"
          name={`${id}-term`}
          options={TERMS.map((t) => ({ value: t, label: TERM_LABELS[t] }))}
          value={term}
          onChange={setTerm}
        />

        <div className="pricing-field">
          <label className="pricing-check" data-locked={supportLocked}>
            <input
              type="checkbox"
              checked={supportLocked || support}
              disabled={supportLocked}
              onChange={(e) => {
                setSupport(e.target.checked);
              }}
              data-testid="pricing-support"
            />
            <span>
              {supportLocked ? (
                <>
                  Support <strong>Included</strong>
                </>
              ) : (
                <>Add support (+{pct(CONFIG.SUPPORT_RATE_BP)})</>
              )}
            </span>
          </label>
        </div>
      </div>

      <section className="pricing-result" aria-label="Your price" data-testid="pricing-result">
        <p className="sr-only" aria-live="polite" aria-atomic="true">
          {liveSummary(result)}
        </p>
        {!result && <p className="story-muted">Enter a record count to see pricing.</p>}
        {result?.status === "contact-sales" && (
          <ContactPanel
            title="Contact sales"
            body={`Above ${formatRecords(CONFIG.MAX_ESTIMATE_RECORDS)} records, we price with you.`}
          />
        )}
        {result?.status === "contact-us" && (
          <ContactPanel
            title="Contact us for pricing"
            body={`Above ${formatRecords(
              CONFIG.tiers[CONFIG.tiers.length - 1]?.maxRecords ?? 0,
            )} records, pricing is set with you.`}
          />
        )}
        {priced && (
          <>
            <div className="pricing-tier">
              <div>
                <p className="story-eyebrow">Your tier</p>
                <p className="pricing-tier-name" data-testid="pricing-tier">
                  {priced.tier.name}
                </p>
                <p className="story-muted">
                  {priced.tier.capRecords === null
                    ? `${formatRecords(priced.records)} records, custom estimate`
                    : `Up to ${formatRecords(priced.tier.capRecords)} records`}
                </p>
              </div>
              <div className="pricing-list-price">
                {priced.estimate ? (
                  <p>
                    Estimated from <strong>{formatUsd(priced.annualListCents)}</strong> per year
                  </p>
                ) : (
                  <p>
                    <strong>{formatUsd(priced.annualListCents)}</strong> per year, list price
                  </p>
                )}
                {priced.estimate && (
                  <Button asChild variant="outline" size="sm" className="mt-2">
                    <a href={CONFIG.SALES_URL}>Talk to sales</a>
                  </Button>
                )}
              </div>
            </div>

            <div className="pricing-selected" data-testid="pricing-selected">
              <p className="pricing-selected-term">{TERM_LABELS[priced.selected]}</p>
              <OptionFacts option={priced.options[priced.selected]} />
              <p className="story-muted pricing-note">
                {optionNote(priced.options[priced.selected])}
              </p>
            </div>

            <ol className="pricing-options" aria-label="Every billing option, cost per year">
              {TERMS.map((t) => {
                const o = priced.options[t];
                return (
                  <li
                    key={t}
                    data-selected={t === priced.selected}
                    data-cheapest={t === priced.cheapest}
                    data-testid={`pricing-option-${t}`}
                  >
                    <div className="pricing-option-head">
                      <span className="pricing-option-name">
                        {TERM_LABELS[t]}
                        {t === priced.selected && <span className="sr-only"> (selected)</span>}
                      </span>
                      {t === priced.cheapest && (
                        <span className="pricing-badge">Lowest per year</span>
                      )}
                      <span className="pricing-option-figure tabular-nums">
                        {formatUsd(o.perYearCents)}
                        <span className="story-muted"> /yr</span>
                      </span>
                    </div>
                    <div className="pricing-bar-track" aria-hidden="true">
                      <span
                        className="pricing-bar"
                        style={
                          {
                            "--bar-w": `${String((o.perYearCents / maxPerYear) * 100)}%`,
                          } as React.CSSProperties
                        }
                      />
                    </div>
                    <p className="pricing-option-meta story-muted tabular-nums">
                      {formatUsd(o.dueAtSignupCents)} at signup · {formatUsd(o.perMonthCents)}/mo ·{" "}
                      {t === "monthly" ? "baseline" : `saves ${String(o.savingsPct)}%`}
                    </p>
                  </li>
                );
              })}
            </ol>

            {priced.competitor && (
              <p className="pricing-competitor" data-testid="pricing-competitor">
                Comparable competitor list price {formatUsd(priced.competitor.competitorCents)} per
                year:{" "}
                {priced.competitor.differencePct <= 0
                  ? `${String(Math.abs(priced.competitor.differencePct))}% less with DupeZero.`
                  : `${String(priced.competitor.differencePct)}% more with DupeZero.`}
              </p>
            )}
          </>
        )}
      </section>

      <SavingsCalculator
        records={parsed.ok ? records : null}
        annualCostCents={priced ? priced.options[priced.selected].perYearCents : null}
        costLabel={priced ? `${priced.tier.name}, ${TERM_LABELS[priced.selected]}` : null}
      />

      <section className="pricing-reference" aria-labelledby={`${id}-grid-title`}>
        <div className="pricing-reference-head">
          <h2 id={`${id}-grid-title`}>Every tier, every term</h2>
          <Segmented
            legend="Show"
            name={`${id}-metric`}
            options={GRID_METRICS.map((m) => ({ value: m, label: GRID_METRIC_LABELS[m] }))}
            value={metric}
            onChange={setMetric}
          />
        </div>
        <div
          className="pricing-table-scroll"
          role="region"
          tabIndex={0}
          aria-labelledby={`${id}-grid-title`}
        >
          <table className="pricing-table" data-testid="pricing-grid">
            <caption className="sr-only">
              {GRID_METRIC_LABELS[metric]} by tier and billing term, {support ? "with" : "without"}{" "}
              support on monthly and yearly terms
            </caption>
            <thead>
              <tr>
                <th scope="col">Tier</th>
                {TERMS.map((t) => (
                  <th scope="col" key={t}>
                    {TERM_LABELS[t]}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {grid.map((row) => (
                <tr key={row.label}>
                  <th scope="row">
                    {row.label}
                    <span className="story-muted"> · {formatRecords(row.records)}</span>
                  </th>
                  {TERMS.map((t) => {
                    const cents = row.cells[t];
                    return (
                      <td key={t} className="tabular-nums">
                        {cents === null ? (
                          <a href={CONFIG.SALES_URL} className="story-text-link">
                            Contact us
                          </a>
                        ) : (
                          formatUsd(cents)
                        )}
                      </td>
                    );
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      <section className="pricing-cta" aria-label="Get started">
        <div className="pricing-cta-buttons">
          <Button asChild size="lg">
            <Link href={CONFIG.TRIAL_URL}>Start {CONFIG.TRIAL_DAYS}-day free trial</Link>
          </Button>
          <Button asChild size="lg" variant="outline">
            <a href={CONFIG.SALES_URL}>Talk to sales</a>
          </Button>
        </div>
        <p className="story-muted">
          Records in free sandbox or test environments do not count toward your tier.
        </p>
        <p className="story-muted" data-testid="pricing-bespoke">
          Need an integration built for your own system? Contact us for a project quote.
        </p>
      </section>

      <ul className="pricing-footnotes story-muted">
        <li>
          Price depends only on your total records, counted across every system you connect,
          Salesforce and any other supported system combined. Records are the rows in the objects
          you choose to scan, such as leads, contacts, accounts and custom objects; activities,
          tasks and other objects we do not process are not counted.
        </li>
        <li>
          Records are counted at signup and again at renewal. In between, each tier has a{" "}
          {pct(CONFIG.SOFT_BUFFER_BP)} soft buffer above its record cap, then we prompt you to
          upgrade. Any true-up happens at renewal, with no surprise mid-term charges.
        </li>
        <li>
          Standard connectors are included at no extra charge, and their setup is part of
          onboarding, which is free on prepaid terms. The number of systems, connectors or orgs
          never changes the price.
        </li>
        <li>Records in free sandbox or test environments do not count toward your tier.</li>
        <li>Every tier includes every feature and unlimited users.</li>
        <li>Prices are in {CONFIG.currency} and subject to change.</li>
      </ul>
    </div>
  );
}
