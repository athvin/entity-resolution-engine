# Rehearsal: connected entity-resolution story

Gold: Stageshow `payout-flow`. Retained cursorless overlapping arrivals, visible
relationships and a persistent payoff. Replaced the payment UI with open system
diagrams at the user's request. The clean hero leads into stacked ingress,
dataset, dedupe and egress panels. Actual node bounds determine the internal
wires and the paths connecting adjacent panels across their boundaries.

The organization owns independent example datasets. Source selections persist per
dataset and volume controls update the illustrative stream. ZoomInfo is labelled
a proposed custom integration in both directions. No price or throughput estimate
is claimed. The separate static Docs route describes the committed contracts and
their gaps.

## Motion

Pipeline marks follow six-second native Web Animation timelines; inter-panel
trails use ten seconds. Shapes distinguish dirty, standardized, matched and
resolved records. Pause, offscreen visibility and hidden tabs hold the timeline.
No per-frame React updates are needed for these paths.

The explicit audit action starts a twelve-second sequence: red flagged match,
evidence, split into two green entities, reconciliation, retained correction.
Labels and particles share one native clock; replay is an explicit action. The
red and green are a user-requested exception scoped to marketing. The static
poster, reduced-motion mode and no-JavaScript rendering retain the explanation.

## Captures and gate

November 9 review artifacts (local, not shipped):

- `/tmp/dz-story-gold.png`: gold reference.
- `/tmp/dz-connected-{flow,dataset,dedupe,egress}-{1440,390}.png`: section views.
- `/tmp/dz-audit-final-{wide,phone}.png`: complete 26-frame contact sheets.
- `/tmp/dz-audit-final-{wide,phone}.json`: instrumentation reports.

Both width runs of `film.mjs --gate` end:

> rehearsal passed (necessary, not sufficient: references/verify.md and the fresh eye in review.md still stand)

Motion sampling remains inconclusive across gaps: the wide capture's largest was
34.7 ms and the phone capture's was 42.1 ms. Those intervals were not judged by
the automated watcher. The entire contact sheets were inspected visually.

The scratch film adapter waits for hydration before adding instrumentation, keeps
the viewport fixed for viewport-height panels, and recognizes the vendored
`stageshow` CSS layer inside Next's combined stylesheet. It does not exempt
product CSS or change the committed skill or vendor bundle.

## Independent review

The fresh-eye reviewer found a discontinuous red-to-green handoff, the audit
heading at a different gutter, and generic entity labels. The red now remains
visible until the greens emerge at the same branch point. The heading and audit
controls align with the earlier panels, and the results identify Meridian US and
Meridian UK. A second review of the final sheets confirmed the handoff and main
alignment fixes. The diagram remains deliberately open and sparse for this
scroll-story foundation; its small caption follows the wider figure gutter.
The reviewer did not establish frame-by-frame quality between samples.

## Validation and limits

Functional tests cover per-dataset source assignments, synchronized volume,
measured node alignment, empty streams, pause/resume, keyboard record inspection,
audit completion, every next-section link, no JavaScript and reduced motion.
Both themes are covered by axe. Linux visual baselines cover each pipeline
section, audit poster, hero, decision dialog, closing CTA and developer docs.

Native scroll snapping must include the closing FAQ and reachable end stops for
tall panels. The audit status reserves its space: inserting it while scrolling
caused WebKit to jump to an earlier snap target. Both failures were reproduced
before fixing them and are exercised by the navigation test.

Physical-device frame delivery and live connector traffic were not tested.
The source icons, pipeline and audit are illustrative. Final gate and independent
review results are recorded in the pull request.

## Maintenance

`node scripts/build-stageshow.mjs` from `frontend/` regenerates the MIT kit bundle
and declarations. Product code remains under strict TypeScript and ESLint.
`node scripts/gen-developer-reference.mjs --check` checks the committed operation
reference against the manifest and response-envelope contract.

## Local mobile interaction revision

The preview is a local production build, served on port 3217. Test the actual
Tailscale origin as well as localhost: the development server's rejected HMR
connection left WebKit's interactive UI unhydrated over the remote HTTP address.
The page looked loaded while its toggle remained disabled and its records never
animated. The production preview hydrates and runs the same bundle on both origins.

The volume controls now share a continuous quadratic position, from 1,000 to
10,000,000 records. Changes retain the flow clock. On a phone the source controls
are a horizontally scrollable row, the slider precedes the diagram, and the
slider's touch handle is larger. A zero minimum inline size on the fieldset is
necessary in Chromium; without it, its min-content width expands the viewport.

Desktop chapter snapping uses one area per section and an end-aligned footer.
On mobile, trail headings and diagrams supply separate stops without an
overlapping parent snap area. Next links supply reachable end stops. Tall chapters remain
scrollable; reduced motion disables snapping and packet animations. The header
is opaque because WebKit showed sharp underlying text through its backdrop blur.
The dark default respects a saved theme, and the header names the destination
mode. Direct touchscreen taps preserve scroll position. Playwright locator.click
can itself scroll a sticky header before clicking; the scroll regression uses
screen coordinates to reproduce the visitor's action.

A diffuse gradient follows the record's real path inside each processing
surface. Material, light, and text occupy separate layers. Every flow surface,
including the selected dataset, is translucent; bridge records continue through
the destination. Text uses foreground contrast above the transmitted light.
It remains an illustration, not a throughput measurement.

Reviewed artifacts are local under `/tmp/landing-approved-*`,
`/tmp/landing-mobile-final-loop.png`, and
`/tmp/landing-desktop-final-loop.png`. The fresh-eye review caught the light
washing out text, Chrome's horizontal overflow, and WebKit's transparent header;
all were corrected. A reported theme jump was reproduced as locator-driven
scrolling and withdrawn after a direct touchscreen tap preserved the viewport.
Rehearsal motion sampling contains gaps; the sheets cannot establish physical
phone frame delivery or exhaustive frame-by-frame smoothness.

## Composited flow motion

The live story holds small HTML sprites beside stationary SVG paths; native
animations change only transform and opacity. SVG foreignObjects are no longer
used for the live story. Gradients replace blur filters, and HTML viewports clip
transmitted light. Coordinates remain local to each SVG viewport, independent
of document scrolling. Normal records and their diffuse echoes share a clock,
including pause and changes in volume. An observer pauses individual offscreen
paths, and rounded bridge bends remove abrupt right-angle direction changes.

A five-second Chromium mobile-emulation trace at 4× CPU throttling recorded
1,505 paint events and 301 layout events before this revision, versus zero of
each after it. Active animations at the dedupe section fell from 32 to 16. Both
traces delivered approximately 60 animation frames per second on this machine;
these measurements establish less rendering work, not physical-phone frame rate.
Functional checks cover offscreen pausing and matching record/echo clocks and
positions on Chromium and WebKit.

The fresh-eye review caught a visible circular edge on the glow. Explicit
`circle closest-side` gradients now reach zero alpha before the rounded sprite
edge. It also measured a 21px native select in WebKit despite its 44px minimum;
explicit height now gives both dataset selectors a 44px touch target, verified
in the interaction suite.

Final local sheets: `/tmp/landing-smooth-{phone,desktop}-loop.png`. Both film
gates passed. The phone watcher had a 29.7ms maximum sampling gap and desktop
62.9ms, so motion between those samples remains inconclusive. Every captured
frame was inspected; gradients stay within the surfaces and text stays readable.
The updated production preview passed 111 marketing checks across desktop
Chromium, mobile Chromium and mobile WebKit. The unit/component coverage run
passed all 922 tests with three workers; an unrestricted concurrent run had
timed out in the unchanged mock-fabrication scanner.

All 27 Linux marketing visual baselines were regenerated and passed comparison.
The final production trace again recorded zero paint/layout events during flow.

## Mobile touch repair

A short native touch swipe was dispatched through Chromium's input protocol,
not simulated with scrollTo. Before the repair it left the processing diagram
at 102px; the updated layout settles it at the 64px header edge. The 320px
viewport also reproduced a 333px document caused by the header; tighter header
spacing fixes it. Both regressions were observed failing against the previous
production preview, then passing against the new build. WebKit tests check
actual packet bounding-box movement as well as animation clocks. Straight paths
use nonzero HTML visibility markers instead of observing zero-width SVG paths.

The independent reviewer watched iPhone 15 and SE at both themes. Light-theme
transmission was too faint, so its foreground-tinted gradient now preserves
contrast beneath pale surfaces. Paired frames confirmed the correction.

The phone and desktop `/tmp/mobile-repair-*-loop.png` sheets were inspected.
Both gates passed; clipped glow reports are intentional masking at processing
surface boundaries, visible in the sheets. Sampling gaps (83.4ms phone, 150.2ms
desktop under concurrent checks) remain inconclusive for motion between frames.
No physical iPhone was connected, so these checks do not establish the user's
device behavior or frame delivery.

Final production validation: 61 interaction checks passed; two CDP-only touch
cases were explicitly skipped outside mobile Chromium. All 27 existing Linux
visual baselines passed unchanged. Build, targeted lint/format, and skills-check
passed. A final throttled Chromium trace retained zero Paint/Layout events and
300 sampled frames in five seconds; physical-device timing remains unmeasured.
