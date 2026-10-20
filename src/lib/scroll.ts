// Remembers where each view was scrolled to, so going back to a list lands where you left it.

const positions = new Map<string, number>();

export function savedScroll(key: string): number {
  return positions.get(key) ?? 0;
}

export function saveScroll(key: string, top: number): void {
  positions.set(key, top);
}
