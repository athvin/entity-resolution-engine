"use client";

import { useEffect, useRef, useState } from "react";
import { ArrowDown, ArrowUp } from "lucide-react";

import { glideTo, HEADER_OFFSET } from "./glide";

/**
 * The story's chapters, in page order. Each `id` is the section's anchor; a
 * chapter added to the page needs a line here too, or the button skips it.
 */
const CHAPTERS = [
  { id: "intro", label: "Start" },
  { id: "flow", label: "Ingress" },
  { id: "dataset", label: "Dataset" },
  { id: "dedupe", label: "Dedupe" },
  { id: "egress", label: "Egress" },
  { id: "audit", label: "Keep the last word" },
  { id: "usage", label: "A workload like yours" },
  { id: "questions", label: "Built to be explained" },
] as const;

/** A chapter whose top is this close to the header counts as already reached. */
const REACHED_SLACK = 8;
/** Below this scroll depth the reader is at the top, and the button stays away. */
const SHOW_AFTER_PX = 120;

/** Index of the first chapter still below the reader, or -1 past the last one. */
function nextChapterIndex(): number {
  const index = CHAPTERS.findIndex(({ id }) => {
    const el = document.getElementById(id);
    return el !== null && el.getBoundingClientRect().top > HEADER_OFFSET + REACHED_SLACK;
  });
  // The last chapter can be too short to reach the top; the page bottom counts.
  const atBottom = window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - 2;
  return atBottom ? -1 : index;
}

/**
 * One floating button that carries the reader forward through the story.
 *
 * It stays away while the reader is at the top of the page. Scrolling down
 * fades it in. Clicking the hero's `[data-explore-flow]` link glides the reader
 * to the flow (see glide.ts), and the button appears once that glide has
 * landed, so the move and the new control arrive one after the other. It always names
 * where it goes next, becomes "Back to top" past the last chapter, and leaves
 * again when the reader returns to the top. The page scrolls freely throughout.
 *
 * The hero link keeps `href="#flow"`, so without JavaScript it is a plain jump.
 * The button is a button, not a `#hash` link: each click would otherwise push
 * a history entry, and Back would walk the reader through every chapter they
 * skipped.
 */
export function StoryChapters() {
  const [next, setNext] = useState(1);
  const [scrolled, setScrolled] = useState(false);
  /** True from an explore click until its glide lands. */
  const [arriving, setArriving] = useState(false);
  const scrolledRef = useRef(false);

  useEffect(() => {
    let frame = 0;
    const update = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        scrolledRef.current = window.scrollY > SHOW_AFTER_PX;
        setScrolled(scrolledRef.current);
        setNext(nextChapterIndex());
      });
    };
    const explore = (event: MouseEvent) => {
      const link =
        event.target instanceof Element ? event.target.closest("[data-explore-flow]") : null;
      if (!(link instanceof HTMLElement)) return;
      event.preventDefault();
      // From the top, the button waits for the glide; once shown, it stays.
      if (!scrolledRef.current) setArriving(true);
      glideTo("flow", () => {
        setArriving(false);
      });
    };
    update();
    window.addEventListener("scroll", update, { passive: true });
    window.addEventListener("resize", update);
    document.addEventListener("click", explore);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
      document.removeEventListener("click", explore);
    };
  }, []);

  const shown = scrolled && !arriving;
  const atEnd = next === -1;
  const target = (atEnd ? undefined : CHAPTERS[next]) ?? CHAPTERS[0];

  const go = () => {
    glideTo(target.id);
  };

  return (
    <div className="story-chapters" data-shown={shown} inert={!shown}>
      <button
        type="button"
        onClick={go}
        className="story-chapters-trigger"
        data-testid="marketing-next-chapter"
        aria-label={atEnd ? "Back to the top of the story" : `Next chapter: ${target.label}`}
      >
        <span className="story-chapters-label">
          {atEnd ? "Back to top" : <>Next: {target.label}</>}
        </span>
        {atEnd ? <ArrowUp aria-hidden="true" /> : <ArrowDown aria-hidden="true" />}
      </button>
    </div>
  );
}
