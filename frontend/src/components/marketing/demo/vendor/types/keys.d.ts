import './keys.css';
/** Where keycaps show. Give `{ wide, compact }` to place them per layout. */
export type KeysAt = 'bottom' | 'top' | 'bottom-left' | 'bottom-right';
export type Keys = {
    id: number;
    caps: string[];
    show: boolean;
    down: boolean;
};
/** 'cmd+k' as caps: ['⌘', 'K']. */
export declare const capsOf: (combo: string) => string[];
/** The last row (fading out) and the current one. */
export declare function Keycaps({ rows, at }: {
    rows: (Keys | null)[];
    at: KeysAt;
}): import("react").JSX.Element;
