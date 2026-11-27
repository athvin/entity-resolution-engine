import type { CSSProperties } from "react";

import { formatRecords, tierFor } from "@/lib/pricing/calculator";
import { PRICING_CONFIG as CONFIG } from "@/lib/pricing/config";

import { ScenePlayback } from "../scene/scene-playback";
import "../scene/scene.css";
import "./records-pool.css";

/**
 * Three connected systems' record counts. Illustrative, and chosen to add up
 * to the calculator's starting count, so the figure and the calculator below
 * it open on the same total.
 */
export const POOL_SOURCES = [
  { key: "salesforce", label: "Salesforce", records: 420_000 },
  { key: "api", label: "API", records: 210_000 },
  { key: "csv", label: "CSV", records: 120_000 },
] as const;

export const POOL_TOTAL = POOL_SOURCES.reduce((sum, source) => sum + source.records, 0);

/**
 * Where a record count sits on a ruler of equal-width tiers: the tier's index
 * plus how far into that tier it is, as a share of the whole ruler.
 */
export function rulerPosition(records: number): number {
  const tiers = CONFIG.tiers;
  const index = tiers.findIndex((tier) => records <= tier.maxRecords);
  if (index < 0) return 1;
  const floor = index === 0 ? 0 : (tiers[index - 1]?.maxRecords ?? 0);
  const within = (records - floor) / ((tiers[index]?.maxRecords ?? records) - floor);
  return (index + within) / tiers.length;
}

/**
 * /pricing's opening figure for the records-only rule: three systems' bars
 * slide end to end into one total, and the total lands on a tier. It sits in
 * the first screen, where a scroll-driven scene would already be finished, so
 * it plays itself once when first seen (ScenePlayback, as the hero does) and
 * can be replayed; its normal styles are the finished picture. The tiers and
 * the tier it lands on come from the pricing config, so it cannot disagree
 * with the calculator.
 */
export function RecordsPool() {
  const tier = tierFor(POOL_TOTAL);
  let offset = 0;
  return (
    <figure className="pool" data-testid="records-pool">
      <div className="pool-stage" id="records-pool-stage" aria-hidden="true">
        <div className="pool-bars">
          <p className="pool-total">
            <strong>{formatRecords(POOL_TOTAL)}</strong> records in total
          </p>
          {POOL_SOURCES.map((source, index) => {
            const share = source.records / POOL_TOTAL;
            const style = {
              "--x": offset,
              "--w": share,
              "--rows-up": POOL_SOURCES.length - 1 - index,
            } as CSSProperties;
            offset += share;
            return (
              <span key={source.key} className="pool-seg" data-index={index} style={style}>
                <strong>{source.label}</strong>
                <span className="pool-seg-count">{formatRecords(source.records)}</span>
              </span>
            );
          })}
        </div>
        <div className="pool-ruler">
          <ol className="pool-tiers">
            {CONFIG.tiers.map((each) => (
              <li key={each.id} data-on={each.id === tier?.id}>
                {each.name}
              </li>
            ))}
          </ol>
          <span
            className="pool-marker"
            style={{ "--p": rulerPosition(POOL_TOTAL) } as CSSProperties}
          />
        </div>
      </div>
      <figcaption className="pool-caption">
        <span>
          Records from Salesforce ({formatRecords(POOL_SOURCES[0].records)}), the API (
          {formatRecords(POOL_SOURCES[1].records)}) and CSV uploads (
          {formatRecords(POOL_SOURCES[2].records)}) add up to one total, and the total alone sets
          the tier: {formatRecords(POOL_TOTAL)} records is {tier?.name ?? "a custom quote"}. The
          calculator below starts there.
        </span>
        <ScenePlayback target="records-pool-stage" label="Replay the records adding up" />
      </figcaption>
    </figure>
  );
}
