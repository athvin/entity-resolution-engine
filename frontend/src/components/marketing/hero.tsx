import Link from "next/link";
import { ArrowDownRight, ArrowUpRight } from "lucide-react";
import { HERO } from "./content";

export function Hero() {
  return (
    <section className="story-hero story-section" data-testid="marketing-hero">
      <div className="story-hero-copy">
        <p className="story-eyebrow">Independent records. A connected world.</p>
        <h1>
          {HERO.headline.split("\n").map((line, i) => (
            <span key={line} className={i === 1 ? "story-muted" : ""}>
              {line}
            </span>
          ))}
        </h1>
        <p className="story-intro">
          Your sources. Your rules.
          <br />
          Records you can trust.
        </p>
      </div>
      <div className="story-hero-foot">
        <p>
          Resolve. Review. Reconcile.
          <br />
          <span className="story-muted">Every connection has a reason.</span>
        </p>
        <div className="story-jump-links">
          <Link href="#flow">
            Explore the flow <ArrowDownRight aria-hidden="true" />
          </Link>
          <Link href="#audit" data-start-audit>
            See how audit works <ArrowUpRight aria-hidden="true" />
          </Link>
        </div>
      </div>
    </section>
  );
}
