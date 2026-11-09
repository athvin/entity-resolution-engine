import { type ReactNode } from 'react';
import './stage.css';
import { type Script } from './act';
import { type CursorKind, type CursorStyle } from './cursor';
import { type KeysAt } from './keys';
import { type Fidelity, type Layout, type Palette, type Theme } from './looks';
import { type Rect, type Target } from './measure';
/** loop: repeats while on screen. once: plays when first seen, then holds the end. hover: plays while the pointer is over it, settles back when it leaves; on a screen with no hover (a phone) it plays once. */
export type Play = 'loop' | 'once' | 'hover';
/** Design tokens for a look: `--ss-accent`, `--ss-radius-md`, `--ss-font` and the rest. */
export type LookTokens = {
    [token: `--ss-${string}`]: string;
};
export interface StageProps<S> {
    /** Design size. The stage scales down to fit its container, and only grows past it with `fill`. */
    width: number;
    height: number;
    /** Scene state at the start of each loop. Statics can leave it out. */
    initial?: S;
    /** Loops while on screen. Leave out for a static illustration. */
    script?: Script<S>;
    /** The still frame shown under reduced motion or with no script. Defaults to initial. */
    poster?: S;
    /**
     * Where the cursor rests at the start of each loop, or where it sits in a
     * still. Leave out to hide it. Animated demos hide it in their reduced-motion
     * poster unless `still` is true. `ripple` sends a soft ring out from each
     * click, for demos where clicks need to read at a glance (small stages, the
     * arrow and circle styles). `idle` is how long (ms) the cursor may rest
     * after it has acted before it fades out (4000 by default); it fades back
     * in as it next moves, and never fades while it presses or hovers. Before
     * its first move each loop it waits out of shot instead, coming back just
     * before it moves. false keeps it on screen throughout. `at` can differ by
     * layout: { wide: ..., compact: ... }, for a stage with a compact design.
     */
    cursor?: {
        at: Target | {
            wide: Target;
            compact: Target;
        };
        kind?: CursorKind;
        style?: CursorStyle;
        still?: boolean;
        ripple?: boolean;
        idle?: number | false;
    };
    play?: Play;
    /**
     * A second, smaller design for narrow containers. Below `below` px of
     * container width the stage uses this size, the render's `layout` argument
     * (and `useLook().layout` inside the stage) and data-layout become 'compact', and the script restarts. Build the
     * compact scene as a tighter crop around the story, never the wide scene
     * shrunk.
     */
    compact?: {
        below: number;
        width: number;
        height: number;
    };
    /** Where keycaps from act.key appear. */
    keys?: KeysAt | {
        wide: KeysAt;
        compact: KeysAt;
    };
    /** Targets shown hovered in a still, since no script is there to hover them. */
    hover?: string[];
    theme?: Theme;
    palette?: Palette;
    fidelity?: Fidelity;
    /** The grays' temperature. `warm` (the default) is a warm canvas and well;
     * `cool` swaps them for cool ones, for a macOS scene or a blue ground, so
     * every gray shares one temperature. Light theme only. */
    greys?: 'warm' | 'cool';
    /** A look as values: any `--ss-*` tokens (color, type, shape, density),
     * set on the stage over its theme's. See `references/looks.md`. */
    tokens?: LookTokens;
    /** Grow past the design size to fill the container (up to 2x), for a hero or a docs plate. A host page can set data-ss-fill on any wrapper instead. */
    fill?: boolean;
    /** The accent color for this stage (any CSS color). Sets --ss-accent, its soft fill and the gradient's second stop together. */
    accent?: string;
    /** What the scene sits on. `orbs`: a pale tile lit by a few huge, soft
     * color orbs behind the UI, so glass has color to take and white cards
     * lift off it. Pass two or three colors to choose them (lavender and
     * orange by default). Leave it out for the plain ground. */
    ground?: 'orbs' | [string, string] | [string, string, string];
    /** Plays the script faster (above 1) or slower (below 1). Waits and tweens divide by it. */
    speed?: number;
    className?: string;
    /** A specimen sheet of parts (the kit's chrome, controls, phones sheets), not a product frame: film's composition checks (framing, ladders, voids) stand down. */
    specimen?: boolean;
    /** No framing, when the developer asks for it: no ground, and the product sits whole with room around it, floating on their page. Film's bleed and floating checks stand down; everything else still applies. */
    unframed?: boolean;
    /** `layout` is 'wide' or 'compact' (see `compact`); useLook() also has it, but only in components rendered inside the stage. */
    children: (s: S, rect: (name: string) => Rect, layout: Layout) => ReactNode;
}
export declare function Stage<S extends object>({ width, height, initial, script, poster, cursor, hover, play: playProp, compact, keys: keysAt, palette, greys, fidelity, specimen, unframed, speed: speedProp, accent, tokens, ground, fill: fillProp, theme: themeProp, className, children, }: StageProps<S>): import("react").JSX.Element;
