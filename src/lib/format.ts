import type { Track } from './types';

/** "3:07", or "1:02:03" from one hour on. Invalid and negative values show "0:00". */
export function formatDuration(ms: number): string {
  const total = Number.isFinite(ms) && ms > 0 ? Math.floor(ms / 1000) : 0;
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, '0');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`;
}

export type QualityInput = Pick<Track, 'format'> &
  Partial<Pick<Track, 'bitDepth' | 'sampleRate' | 'bitrateKbps'>>;

const LOSSLESS = new Set(['flac', 'wav', 'aiff', 'aif', 'alac', 'ape', 'wv', 'dsf', 'dff']);
const NAMES: Record<string, string> = { aif: 'AIFF', oga: 'OGG', wv: 'WAVPACK' };

/** 96000 -> "96", 44100 -> "44.1". */
function khz(hz: number): string {
  return String(Math.round(hz / 100) / 10);
}

/** "FLAC 24/96", "MP3 320", "OPUS". The format is the file extension the scanner stored. */
export function qualityLabel(track: QualityInput): string {
  const format = track.format.toLowerCase();
  const { bitDepth, sampleRate, bitrateKbps } = track;
  // m4a holds either ALAC (which reports a bit depth) or AAC.
  const lossless = LOSSLESS.has(format) || (format === 'm4a' && !!bitDepth);
  const name = format === 'm4a' ? (lossless ? 'ALAC' : 'AAC') : (NAMES[format] ?? format.toUpperCase());

  if (lossless) {
    return bitDepth && sampleRate ? `${name} ${bitDepth}/${khz(sampleRate)}` : name;
  }
  return bitrateKbps ? `${name} ${Math.round(bitrateKbps)}` : name;
}
