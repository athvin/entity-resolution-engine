export type Bezier = readonly [number, number, number, number];
export declare const EASE: {
    /** Moves inside a shot (push-in, pan, a long cursor trip): peak at about 40%, long soft tail. 1.4 to 2.7 s. */
    readonly house: readonly [0.65, 0, 0.1, 1];
    /** The outgoing half of a move that ends in a cut: accelerates into it. */
    readonly through: readonly [0.8, 0, 0.1, 1];
    /** The incoming half after a cut: arrives at speed, settles. 0.6 to 0.8 s (1.6 s for a big pull-out). */
    readonly settle: readonly [0.05, 0.85, 0.3, 1];
    /** A cursor arriving from off-frame, already moving. */
    readonly enter: readonly [0.1, 0.8, 0.3, 0.95];
    /** One long cursor trip across the frame. */
    readonly trip: readonly [0.78, 0, 0.17, 1];
    /** The kit's camera push-in (Camera). */
    readonly camera: readonly [0.45, 0, 0.1, 1];
};
export type EaseName = keyof typeof EASE;
export declare const css: (e: EaseName | Bezier) => string;
/** cubic-bezier(x1, y1, x2, y2) as a function of time 0..1, as CSS works it out. */
export declare function bezier(e: EaseName | Bezier): (t: number) => number;
/** The curve's slope as it starts (progress per unit time at t = 0). */
export declare const startSlope: (e: EaseName | Bezier) => number;
