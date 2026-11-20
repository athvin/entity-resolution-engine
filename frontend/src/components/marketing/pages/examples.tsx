/**
 * Example panels for the content pages, in place of screenshots: what the
 * review and source-mapping screens show, drawn in HTML with fictional
 * records and labelled "Example". Agreement is carried by words and weight,
 * never colour, so the panels follow the grayscale rule.
 */

const PAIR = [
  {
    field: "Name",
    a: "Liz Moreno",
    b: "Elizabeth Moreno",
    result: "Nickname match",
    weight: "+3.2",
  },
  {
    field: "Email",
    a: "liz.moreno@harbourline.example",
    b: "LIZ.MORENO+events@harbourline.example",
    result: "Same after cleanup",
    weight: "+6.8",
  },
  {
    field: "Company",
    a: "Harbourline Ltd",
    b: "Harbourline",
    result: "Same, suffix ignored",
    weight: "+2.1",
  },
  { field: "Phone", a: "(415) 555-0142", b: "415.555.0199", result: "Different", weight: "−1.4" },
] as const;

export function ExampleReviewPair({ label }: { label: string }) {
  return (
    <figure className="page-example" aria-label={label}>
      <figcaption>
        <span className="page-example-tag">Example</span> A pair waiting for review
      </figcaption>
      <div className="page-example-scroll">
        <table>
          <thead>
            <tr>
              <th scope="col">Field</th>
              <th scope="col">Record A</th>
              <th scope="col">Record B</th>
              <th scope="col">Evidence</th>
              <th scope="col" className="page-example-num">
                Counted
              </th>
            </tr>
          </thead>
          <tbody>
            {PAIR.map((row) => (
              <tr key={row.field} data-differs={row.result === "Different"}>
                <th scope="row">{row.field}</th>
                <td>{row.a}</td>
                <td>{row.b}</td>
                <td>{row.result}</td>
                <td className="page-example-num">{row.weight}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="page-example-foot">
        <span>Three fields agree, one differs.</span>
        <span className="page-example-actions" aria-hidden="true">
          <span>Same person</span>
          <span>Not the same</span>
        </span>
      </p>
    </figure>
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
      <div className="page-example-scroll">
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
      </div>
    </figure>
  );
}
