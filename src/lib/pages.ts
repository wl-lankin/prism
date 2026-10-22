// Paging for long lists: which pages a row range needs, and which cached pages to drop.

/** The pages of `pageSize` rows that rows `start` (inclusive) to `end` (exclusive) touch. */
export function pagesFor(start: number, end: number, pageSize: number): number[] {
  if (end <= start) return [];
  const out: number[] = [];
  for (let page = Math.floor(start / pageSize); page <= Math.floor((end - 1) / pageSize); page++) out.push(page);
  return out;
}

/**
 * The cached pages to evict so at most `max` remain: the ones farthest from the nearest
 * `visible` page go first (ties: the lower page first). Visible pages are never evicted.
 */
export function evictPages(cached: Iterable<number>, visible: number[], max: number): number[] {
  const pages = [...cached];
  if (pages.length <= max) return [];
  const keep = new Set(visible);
  const distance = (page: number) =>
    visible.length ? Math.min(...visible.map((v) => Math.abs(v - page))) : page;
  const candidates = pages
    .filter((page) => !keep.has(page))
    .sort((a, b) => distance(b) - distance(a) || a - b);
  return candidates.slice(0, Math.max(0, pages.length - max));
}
