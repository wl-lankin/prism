/** Where a context menu opens: at the pointer for a right-click on a row, or just below the
 *  selected row when the menu key or Shift+F10 opened it (the event then targets the list). */
export function menuPoint(event: MouseEvent, row: Element | null): { x: number; y: number } {
  const onRow = !!(event.target as Element | null)?.closest?.('[data-index]');
  const pointed = onRow && (event.clientX !== 0 || event.clientY !== 0);
  if (pointed || !row) return { x: event.clientX, y: event.clientY };
  const box = row.getBoundingClientRect();
  return { x: box.left + 56, y: box.bottom - 4 };
}
