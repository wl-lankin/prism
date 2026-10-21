import { SvelteSet } from 'svelte/reactivity';
import { setOverlayOpen } from './ytm.svelte';

/** Ids of the overlays that are open (search palette, context menus, dialogs, About, the
 *  Flow menu). The YouTube Music view is a native view above the page, so it is hidden
 *  while any of them is open. An overlay opens on mount and closes on unmount. */
const open = new SvelteSet<string>();

export function openOverlay(id: string): void {
  const was = open.size > 0;
  open.add(id);
  if (!was) setOverlayOpen(true);
}

export function closeOverlay(id: string): void {
  if (!open.delete(id)) return;
  if (open.size === 0) setOverlayOpen(false);
}

/** True while at least one overlay is open. */
export const anyOverlay = (): boolean => open.size > 0;

export const overlayOpen = (id: string): boolean => open.has(id);
