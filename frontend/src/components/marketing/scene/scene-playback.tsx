"use client";

import { useEffect, useState } from "react";
import { RotateCcw } from "lucide-react";

/**
 * Playback for a self-playing scene whose motion is plain CSS: the scene stays
 * a server component with no script, and this is its only island.
 *
 * Two jobs. If the stage is not on screen when the page hydrates (a short
 * phone, where the hero scene sits below the way into the story), its
 * animations are held at the start and run when it is first seen, so nobody
 * scrolls down to a scene that already finished. And Replay runs it again.
 *
 * Without JavaScript the CSS simply plays on load. The button renders only
 * after hydration, so a reader without JavaScript never meets one that does
 * nothing; reduced motion hides it in CSS, because nothing moves.
 */
export function ScenePlayback({ target, label }: { target: string; label: string }) {
  const [ready, setReady] = useState(false);

  useEffect(() => {
    setReady(true);
    const stage = document.getElementById(target);
    if (!stage || typeof IntersectionObserver === "undefined") return;
    const box = stage.getBoundingClientRect();
    if (box.top < window.innerHeight && box.bottom > 0) return;
    const animations = stage.getAnimations({ subtree: true });
    for (const animation of animations) {
      animation.pause();
      animation.currentTime = 0;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (!entries.some((entry) => entry.isIntersecting)) return;
        for (const animation of animations) animation.play();
        observer.disconnect();
      },
      { threshold: 0.4 },
    );
    observer.observe(stage);
    return () => {
      observer.disconnect();
    };
  }, [target]);

  if (!ready) return null;
  return (
    <button
      type="button"
      className="scene-replay"
      aria-label={label}
      data-testid={`${target}-replay`}
      onClick={() => {
        document
          .getElementById(target)
          ?.getAnimations({ subtree: true })
          .forEach((animation) => {
            animation.cancel();
            animation.play();
          });
      }}
    >
      <RotateCcw aria-hidden="true" />
      Replay
    </button>
  );
}
