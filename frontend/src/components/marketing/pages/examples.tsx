import Image from "next/image";

/**
 * Real screens from the DupeZero app, rendered from this codebase.
 *
 * The images are the desktop visual baselines the Playwright suite renders in
 * CI (tests/functional/{reviews,records}.spec.ts-snapshots), copied to
 * public/screenshots/. They show the test fixtures' sample data, never customer
 * data. When the app's screens change, re-copy the matching baseline.
 */
const SCREENS = {
  review: {
    src: "/screenshots/review-queue.png",
    width: 1440,
    height: 1110,
    alt: "The DupeZero review screen: a queue of possible duplicates on the left, and one pair compared field by field on the right, with the fields that differ flagged and the weight each agreement added to the match.",
    caption:
      "The review screen. Each pair shows which fields agree, which differ, and how much each counted.",
  },
  golden: {
    src: "/screenshots/golden-record.png",
    width: 1440,
    height: 1334,
    alt: "A DupeZero golden record: each value with the source record and rule it came from, the values each member record disagrees on, the member records, and the history of runs that formed it.",
    caption: "A golden record. Every value shows where it came from, and the history is kept.",
  },
} as const;

export function ProductScreenshot({ screen }: { screen: keyof typeof SCREENS }) {
  const s = SCREENS[screen];
  return (
    <figure className="page-screenshot">
      <Image
        src={s.src}
        alt={s.alt}
        width={s.width}
        height={s.height}
        sizes="(max-width: 800px) 100vw, 1100px"
        loading="lazy"
      />
      <figcaption className="story-muted">
        {s.caption} <span>Sample data.</span>
      </figcaption>
    </figure>
  );
}

/**
 * Example panel for the content pages, where no clean app screen exists: what the
 * source-mapping screen shows, drawn in HTML with fictional
 * records and labelled "Example". Agreement is carried by words and weight,
 * never colour, so the panels follow the grayscale rule.
 */

/** The table scrolls sideways on a phone, so it takes focus and a name (axe: scrollable-region-focusable). */
function Scroll({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="page-example-scroll" role="region" aria-label={label} tabIndex={0}>
      {children}
    </div>
  );
}

const MAPPING = [
  { column: "First Name", field: "First name" },
  { column: "Last Name", field: "Last name" },
  { column: "Email", field: "Email" },
  { column: "Account Name", field: "Company" },
  { column: "Mailing Street", field: "Address line" },
  { column: "Lead Source", field: "Not used for matching" },
] as const;

export function ExampleMapping({ label }: { label: string }) {
  return (
    <figure className="page-example" aria-label={label}>
      <figcaption>
        <span className="page-example-tag">Example</span> Mapping a new source
      </figcaption>
      <Scroll label="Example field mapping, table">
        <table>
          <thead>
            <tr>
              <th scope="col">Column in your data</th>
              <th scope="col">DupeZero field</th>
            </tr>
          </thead>
          <tbody>
            {MAPPING.map((row) => (
              <tr key={row.column} data-differs={row.field.startsWith("Not")}>
                <th scope="row">{row.column}</th>
                <td>{row.field}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </Scroll>
    </figure>
  );
}
