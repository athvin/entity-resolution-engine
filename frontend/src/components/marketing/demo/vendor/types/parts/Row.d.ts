import type { HTMLAttributes, ReactNode } from 'react';
import './Row.css';
export interface RowProps extends Omit<HTMLAttributes<HTMLDivElement>, 'title'> {
    /** The disc on the left: an `Avatar`, an icon tile, a thumbnail. */
    lead?: ReactNode;
    /** The first line: a name, a title. A `Text` or a `Bar`. */
    title: ReactNode;
    /** The second line: a preview, a description. A `Text` or a `Bar`. */
    subtitle?: ReactNode;
    /** Trailing detail on the first line: a time, a count, a badge. */
    meta?: ReactNode;
    /** Preview lines under the subtitle: bar widths in px for placeholder (Mail's two-line preview: `[220, 150]`), or the real text. */
    preview?: number[] | ReactNode;
    /** An unread dot hanging in the margin left of the lead; the row never moves. */
    unread?: boolean;
    selected?: boolean;
    /** Inset of the row's fill, in px. 12 by default; a dense list takes 8. */
    pad?: number;
}
/**
 * One row of a list: an inbox conversation, a sidebar item, a table line. The
 * anatomy is fixed so a list reads as a product and not a wireframe: the lead
 * centered on the two lines, the title 4 to 6px above its subtitle (the air
 * comes from the row's padding, not the gap), the meta on the first line, and
 * an unread dot hanging in the margin, so the row never moves. A lead and the dot
 * sit on the name's two lines, not the middle of a taller row; anything
 * passed as children (a toggle, a chevron) trails at the end of that line.
 * A title with no subtitle is a one-line row: the title plus the padding, no
 * second line reserved, so a file or settings list stays slim.
 * More lines under the subtitle (preview bars) go inside `subtitle`. Selected and hovered
 * rows take an inset, rounded fill. Build lists out of it instead of by hand.
 */
export declare function Row({ lead, title, subtitle, meta, preview, unread, selected, pad, className, style, children, ...rest }: RowProps): import("react").JSX.Element;
