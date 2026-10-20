// Windowing for long lists: only the rows near the viewport are rendered.

/**
 * The rows to render for a list of `total` rows of `rowHeight` px scrolled to `scrollTop` in a
 * `viewport` px high window: every row at least partly visible plus `overscan` rows on each
 * side. `end` is exclusive and never past `total`.
 */
export function visibleRange(
  scrollTop: number,
  viewport: number,
  rowHeight: number,
  total: number,
  overscan: number,
): { start: number; end: number } {
  if (total <= 0 || rowHeight <= 0) return { start: 0, end: 0 };
  const top = Math.max(0, scrollTop);
  const first = Math.floor(top / rowHeight);
  const last = Math.ceil((top + Math.max(0, viewport)) / rowHeight);
  const start = Math.min(total, Math.max(0, first - overscan));
  const end = Math.min(total, last + overscan);
  return { start, end: Math.max(start, end) };
}

/**
 * Columns for a card grid `width` px wide: as many cards of at least `minCard` px as fit with
 * `gap` px between them, stretched to fill the row. Always at least one column.
 */
export function gridLayout(width: number, minCard: number, gap: number): { columns: number; cardWidth: number } {
  const columns = Math.max(1, Math.floor((width + gap) / (minCard + gap)));
  const cardWidth = Math.max(0, (width - (columns - 1) * gap) / columns);
  return { columns, cardWidth };
}

/**
 * Where to scroll a card grid after its column count changed so the first visible card stays
 * first: `offset` px into the old grid (rows of `oldRow` px, `oldColumns` wide) becomes the
 * offset of that card's new row, keeping the same fraction of the row scrolled past.
 */
export function anchorOffset(offset: number, oldRow: number, oldColumns: number, newRow: number, newColumns: number): number {
  if (offset <= 0 || oldRow <= 0 || newRow <= 0) return Math.max(0, offset);
  const row = Math.floor(offset / oldRow);
  const within = (offset - row * oldRow) / oldRow;
  const first = row * oldColumns;
  return (Math.floor(first / newColumns) + within) * newRow;
}
