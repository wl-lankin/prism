// The queue panel's list arithmetic: where it starts and how a drag moves rows.

/** The localStorage key that remembers whether the queue panel is open. */
export const QUEUE_PANEL_KEY = 'prism.queuePanel';

/** The id of the transport strip's queue toggle: focus returns there when the panel closes. */
export const QUEUE_TOGGLE_ID = 'queue-toggle';

/** A drag in progress: the row at `from` is `dy` px off its place and would land on `to`. */
export interface QueueDrag {
  from: number;
  to: number;
  dy: number;
}

/** The first queue index the list shows: the current entry, or the first one when the
 *  played entries are shown too. */
export function firstRow(current: number, total: number, showPlayed: boolean): number {
  return showPlayed ? 0 : Math.max(0, Math.min(current, total));
}

/** Where a row dragged `dy` px from `from` lands, in whole rows of `row` px, kept within
 *  `min`..`max` (the upcoming entries). */
export function dragTarget(from: number, dy: number, row: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, from + Math.round(dy / row)));
}

/** How far row `i` is drawn off its place while `drag` is on: the dragged row follows the
 *  pointer, the rows it passed step aside by one row. */
export function stepAside(i: number, drag: QueueDrag | null, row: number): number {
  if (!drag) return 0;
  const { from, to, dy } = drag;
  if (i === from) return dy;
  if (from < to && i > from && i <= to) return -row;
  if (to < from && i >= to && i < from) return row;
  return 0;
}
