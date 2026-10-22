import { SvelteSet } from 'svelte/reactivity';

/** Ids of the overlays that are open (search palette, context menus, dialogs, About, the
 *  Flow menu). The YouTube Music view is a native view above the page, so it is hidden
 *  while any of them is open. An overlay opens on mount and closes on unmount. */
const open = new SvelteSet<string>();

/** Told when the first overlay opens and the last one closes. The YouTube Music view sets it
 *  while it is mounted; the registry itself knows nothing about that view, so a window that
 *  never shows it (the mini player) never reaches its commands. */
let listener: ((open: boolean) => void) | null = null;

export function setOverlayListener(next: ((open: boolean) => void) | null): void {
  listener = next;
  if (next && open.size > 0) next(true);
}

export function openOverlay(id: string): void {
  const was = open.size > 0;
  open.add(id);
  if (!was) listener?.(true);
}

export function closeOverlay(id: string): void {
  if (!open.delete(id)) return;
  if (open.size === 0) listener?.(false);
}

/** True while at least one overlay is open. */
export const anyOverlay = (): boolean => open.size > 0;

export const overlayOpen = (id: string): boolean => open.has(id);
