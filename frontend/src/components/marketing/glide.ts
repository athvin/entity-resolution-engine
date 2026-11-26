/**
 * An eased, interruptible scroll to a story section.
 *
 * Native `scroll-behavior: smooth` leaves the curve and speed to the browser,
 * and story.css turns it off entirely on touch devices, so a jump there is a
 * cut. This gives every device the same motion: ease-in-out, with a duration
 * that grows with distance inside a fixed band, so a short hop is not sluggish
 * and a long one is not a blur.
 *
 * The page scrolls freely, so the reader always wins: any wheel, touch, pointer
 * or key input during a glide stops it where it is.
 */

/** Matches `scroll-padding-top` in story.css: the sticky header's height. */
export const HEADER_OFFSET = 64;

const MIN_MS = 550;
const MAX_MS = 1100;
/** Milliseconds per pixel travelled, before clamping into the band above. */
const MS_PER_PX = 0.45;
const INTERRUPTS = ["wheel", "touchstart", "pointerdown", "keydown"] as const;

function easeInOutCubic(t: number): number {
  return t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2;
}

let active: (() => void) | null = null;

/**
 * The target's `scroll-margin-top`, which native fragment navigation honours
 * and a glide should too: a pinned scene's beat uses it to land its card in
 * the middle of the visible band, on the frame the scene draws for it.
 */
export function scrollMarginTop(target: Element): number {
  return Number.parseFloat(getComputedStyle(target).scrollMarginTop) || 0;
}

/**
 * Glide to the element with `id`, then record it in the URL (replacing, not
 * pushing, so Back still leaves the page) and give it focus without scrolling,
 * which is where native fragment navigation would leave keyboard users.
 * `onDone` runs when the glide lands or is interrupted.
 */
export function glideTo(id: string, onDone?: () => void): void {
  const target = document.getElementById(id);
  active?.();
  if (!target) {
    onDone?.();
    return;
  }

  const finish = (landed: boolean) => {
    active = null;
    for (const type of INTERRUPTS) window.removeEventListener(type, interrupt);
    if (landed) {
      history.replaceState(history.state, "", `#${id}`);
      if (!target.hasAttribute("tabindex")) target.setAttribute("tabindex", "-1");
      target.focus({ preventScroll: true });
    }
    onDone?.();
  };
  let frame = 0;
  const interrupt = () => {
    cancelAnimationFrame(frame);
    finish(false);
  };

  const from = window.scrollY;
  const max = document.documentElement.scrollHeight - window.innerHeight;
  const to = Math.min(
    max,
    Math.max(
      0,
      target.getBoundingClientRect().top + from - HEADER_OFFSET - scrollMarginTop(target),
    ),
  );
  const distance = Math.abs(to - from);
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  if (reduced || distance < 2) {
    window.scrollTo({ top: to, behavior: "instant" });
    finish(true);
    return;
  }

  const duration = Math.min(MAX_MS, Math.max(MIN_MS, distance * MS_PER_PX));
  const start = performance.now();
  const step = (now: number) => {
    const t = Math.min(1, (now - start) / duration);
    // `instant` overrides the stylesheet's smooth scroll, which would
    // otherwise animate each frame's small step and fight this curve.
    window.scrollTo({ top: from + (to - from) * easeInOutCubic(t), behavior: "instant" });
    if (t < 1) frame = requestAnimationFrame(step);
    else finish(true);
  };
  for (const type of INTERRUPTS) window.addEventListener(type, interrupt, { passive: true });
  active = interrupt;
  frame = requestAnimationFrame(step);
}
