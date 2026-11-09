# DupeZero flow illustrations

## Story and framing

The headline remains **Entity resolution you can defend.** Resolve, Review,
Reconcile are the core principles. The hero demonstrates a complete system:
Google Sheets, Salesforce, SFTP, S3, Shopify and a custom API feed configurable
datasets; standardization and evidence-based matching produce consolidated or
independent entities; golden records fan out to destinations.

Ingress, Northstar and egress have separate visual boundaries. Northstar is the
example organization containing DupeZero and its datasets. Commerce, Operations
and Shared index demonstrate overlapping source assignments and independent
matching rules. Dataset selection highlights the relevant routes without
reframing or scrolling the page. The separate review illustration demonstrates
configuration, AI-assisted review, human approval and unmerge.

The Stageshow choreography reference is `payout-flow`: overlapping arrivals,
visible relationships and a persistent payoff. These are open system diagrams
at the user's request. Desktop uses a horizontal graph; compact uses a vertical
layout with reserved geometry. Kit text treatments, existing Lucide icons and
product buttons inherit the application's grayscale tokens.

## Motion and interaction

The main flow repeats on six-second timelines, with eight or twelve staggered
marks on the busiest routes. Shape carries state: dashed diamond for dirty,
outline circle for standardized, linked circles for matched, filled circle for
deduped and outline square for alone. Matching branches visibly converge; the
unmatched branch stays separate. A persistent key explains all five states.

SVG path geometry is sampled once into native Web Animations keyframes. There
are no per-frame React updates. Invisible packets cannot intercept pointer
input. Hover holds the streams, double-click opens the record decision, and tap
provides the equivalent touch action. The legend and toolbar provide stable
keyboard-accessible inspection controls. A dismissible dialog shows the selected
dataset, before/after values and decision evidence.

Audit diverts a consolidated record, separates it into two marks and returns
them to reconciliation. The example retains the correction and its explanation.
An explicit audit action scrolls its branch into view. Review policy examples
remain in the separate review flow: configuration proceeds automatically,
AI-assisted review requires accepting a recommendation, and human review waits
for approval. These are illustrative decisions, not live integrations or
measured performance claims.

Pause, offscreen visibility and background tabs pause native timelines. Reduced
motion and no-JavaScript rendering preserve a complete static diagram. Audit
phase timers use reduced-motion shortcuts; the normal audit narrative uses a
separate timer from packet visibility.

## Review evidence

Artifacts from the November 8 revision are local review evidence:

- `/tmp/dz-final-pipeline-chromium-{1440,390}.png`: desktop/phone composition.
- `/tmp/dz-final-dialog-{chromium,webkit}-390.png`: phone inspection dialog.
- `/tmp/dz-record-film-{wide,phone}.png`: motion contact sheets.
- `/tmp/dz-record-final-phone.png`: compact polish contact sheet.
- `/tmp/dz-record-continuity.json`: deterministic timeline sampling.

The independent reviewer exercised dataset selection, physical double-click and
touch on moving/paused records, decision evidence and the audit return. Two
findings were fixed: invisible packets intercepted clicks, and phone microcopy
was too small. Phone captions now use at least 10px.

The film instrumentation has sampling gaps under screenshot load, so its motion
judgment is inconclusive. Its zone-padding warning treats the organization
boundary as a small card; its egress-stack warning ignores the record marks
between title and caption. These intentional diagram structures were checked
visually. Transparent hit circles and invisible timeline endpoints contributed
to earlier clipped-glyph reports; endpoints now remain on their actual path.
Actual source, processing and audit padding findings were corrected. The final
desktop film reports only the organization-boundary padding heuristic; the phone
film also reports the intentional egress title/marks/caption stack. Deterministic
16ms sampling across two cycles found no path-speed discontinuities or invisible
click targets among 232 rendered packets at either width.

Rehearsal uses a scratch adapter, without editing the vendored skill. It recognizes
the upstream CSS layer in Next's combined stylesheet and waits for hydration
before attaching instrumentation. Physical-device frame delivery, live source
connections and production deployment were not tested.

## Validation

Functional coverage includes dataset/source routing, stable geometry, dense
streams and pause/resume, physical record inspection, keyboard dialog access,
audit diversion/return, review approval and unmerge, reduced motion, no JavaScript,
and accessibility in both themes. Linux visual baselines cover the hero,
decision dialog, core principles, provenance, unmerge and closing CTA.

The decision dialog explicitly focuses its heading without scrolling, preventing
mobile autofocus from skipping the beginning of the evidence. Final gate results
are reported with the implementation. The production build
must retain static prerendering for `/`.

## Maintenance

`node scripts/build-stageshow.mjs` from `frontend/` regenerates the MIT kit bundle
and declarations. Product code remains under strict TypeScript and ESLint. The
social card source is `frontend/assets/marketing-social.svg`; its committed PNG
is `frontend/src/app/opengraph-image.png`.
