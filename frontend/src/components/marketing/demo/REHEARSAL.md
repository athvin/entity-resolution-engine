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

Scroll snapping has since been removed by the owner's decision: the story
scrolls freely and nothing intercepts wheel or touch (both asserted in
`marketing-interactions.spec.ts`). The audit status still reserves its space,
which it first did because inserting it while scrolling made WebKit jump to an
earlier snap target.

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

This revision used chapter snapping, since removed (see above); reduced motion
still disables packet animations. The header is opaque because WebKit showed
sharp underlying text through its backdrop blur.
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

## Hero opening scene and the #dedupe set piece (epic #1253)

Two scenes follow Meridian Supply, the entity the story already resolves to
E-1042: the hero's self-playing opening (#1255) and the pinned, scroll-scrubbed
`#dedupe` set piece (#1256). Gold references: `stacked-cards` and
`tilted-stack` for the hero's payoff, `payout-flow` for overlapping beats that
hold their payoff.

**Hero.** It was filmed by freezing every animation at twelve times between
100ms and 4.9s and capturing the hero on desktop Chromium (1440x900) and on
WebKit with the iPhone 15 profile, plus the reduced-motion poster. Three rounds:

- **Round one.** The cards were too small for the space. The evidence line ran
  through the cards, because the minifier had dropped its `translate`. The
  payoff's stack was invisible behind the golden record.
- **Round two.** Cards grew to `min(330px, 62cqw)`, the stack moved to
  peek-above strips, and the line went through `var()`. On phones, though, the
  spread ran off the left edge and the golden record's email broke mid-word.
- **Round three.** Added a container-scoped spread factor and labels above
  values. The spec then failed for real on phones: the iPhone 15 profile shows
  659px. The scene moved after the foot, and `scene-playback.tsx` now holds it
  until it is first seen.

**Set piece.** It was filmed by scrolling the track to twelve progress points
from 0 to 100%, on the same two profiles. Round one showed a muddy frame at
84%, where the golden record faded in over the fading records, and on phones
the decision landed on top of the evidence. The beats were re-sequenced so each
moment has the frame to itself, and round two was clean.

`film.mjs --gate` samples on a clock and cannot drive a scroll timeline, so
these sheets are the scrub's rehearsal. They were judged by eye against
`references/framing.md` and `craft.md`, not by the automated watcher, and
physical-device frame delivery was not measured. The functional specs assert
what the sheets showed, and each was seen failing first:

- the scene's duration (Received: 5620 when mutated to 5.4s);
- reduced motion (19 animations without the gate);
- scrub state (the golden record at opacity 1 with its animation removed);
- chapter landing (338px off-centre without the scroll margin).

## The home audit film (#1297)

Section 06 is now a film the visitor takes part in. `scene/audit-desk.tsx`
draws it and `demo/audit-story.tsx` times it. Gold references:

- `payout-flow`, for arrivals that overlap and a payoff that holds;
- `table-edit`, for an edit made in place and then the total it feeds;
- `form-error`, for a fault on the thing that caused it, cleared by fixing it.

**How it was filmed.** A probe drove the real page: it brought the stage into
view, took frames at fixed times through each moment, and clicked the decisions
itself. Both branches were filmed on desktop Chromium (1280x800) and the Pixel
7 profile. Four rounds:

- **Round one.**
  - The four shots share one grid cell, so the shorter ones (the zone, the
    record view, the next run) sat over empty bands.
  - A match left an empty column with an "= Always match" chip floating in it.
  - The unmerge message landed under the departing row, across a gap.
  - The never-match chip covered a card's label.
  - The controls hid under the floating next-chapter button.
- **Round two.**
  - The zone gained a legend saying what each zone does, and centres in its
    frame.
  - A match folds the API card under the Salesforce card and uncovers E-1042's
    golden record, opaque and dealt from under.
  - The record view shows the record's rules. The unmerge's never-match rule
    arrives as the row leaves, and the applied message replaces the row in
    place.
  - The never mark became a round ⊘.
  - The controls moved under the status, on the left.
- **Round three, caught by the functional spec.** On phones the window
  changed height by 87px between shots, because the record view and the next
  run drew some rows only in some states. The same 87px broke the footer spec
  on WebKit, because the status bar also grew at hydration. Now every shot
  draws one structure in every state. The status line reserves its longest
  sentence, and the controls row reserves one button.

- **Round four, WebKit only, also first seen as the footer spec failing.**
  With the reader already parked at the very bottom of the page, hydration
  moved the scroll by 104px. The page height did not change, no script
  scrolled, and `overflow-anchor: none` made no difference. Bisecting with
  injected styles found two element swaps: 56px from the step rail (a `<span>`
  became a `<button>`) and 48px from the London row (a `<span>` became a
  `<label>`). When an in-flow element is replaced, WebKit clamps a reader at
  the maximum scroll position; pure insertions into reserved space did nothing.
  - The rail keeps its words, and a button is laid over them after hydration.
  - The row is a `<label>` from the server render on, and only its 18px box
    is swapped.
  - The film stays the still poster until it comes within a screen of the
    viewport, so it no longer changes state at hydration either.

  A step change with the reader parked at the bottom still moves WebKit (forced
  from a script in a probe). No reader can do that: the clock pauses with the
  section off screen, and the rail is only reachable in view.

**What was not done.** `film.mjs --gate` was not run: it needs
`playwright-core` beside the skill, and its clock sampling cannot make the
visitor's decisions. The sheets were judged by eye against
`references/framing.md` and `craft.md`. Frame delivery on a physical device
was not measured.
