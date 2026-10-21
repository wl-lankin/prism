import { t } from '../i18n';
import { notify } from '../stores/notice.svelte';
import { playStation } from '../stores/radio.svelte';
import type { Station } from '../types';

/** The quiet second line of a station row: where it is from, else its first tag. */
export function stationSub(station: Station): string {
  const place = [station.region, station.countryCode].filter(Boolean).join(', ');
  return place || station.tags[0] || '';
}

/** Plays a station; when that fails the notice names it and carries the backend's message. */
export async function playStationNotified(station: Station): Promise<void> {
  try {
    await playStation(station);
  } catch (err) {
    const message = typeof err === 'string' ? err : err instanceof Error ? err.message : String(err);
    notify(t('stations.playFailed', { name: station.name, message }));
  }
}
