import type { Source, StreamQuality, Track } from './types';

/** "3:07", or "1:02:03" from one hour on. Invalid and negative values show "0:00". */
export function formatDuration(ms: number): string {
  const total = Number.isFinite(ms) && ms > 0 ? Math.floor(ms / 1000) : 0;
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, '0');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`;
}

/** A station: radio with no duration (the engine's `is_live`). */
export function isLive(track: Pick<Track, 'source' | 'durationMs'> | null | undefined): boolean {
  return track?.source === 'radio' && track.durationMs === 0;
}

/** The two lines a station shows: the stream title over the station name, or, without a
 *  title, the station name over its region line (the track's artist). */
export function liveLines(track: Pick<Track, 'title' | 'artist'>, streamTitle: string | null): { title: string; sub: string } {
  const heard = streamTitle?.trim();
  return heard ? { title: heard, sub: track.title } : { title: track.title, sub: track.artist };
}

/** "Radio Testhausen" -> "RT": up to two capitals for a station without a logo. */
export function stationInitials(name: string): string {
  const words = name.split(/\s+/).filter((w) => /[\p{L}\p{N}]/u.test(w));
  return words
    .slice(0, 2)
    .map((w) => ([...w].find((c) => /[\p{L}\p{N}]/u.test(c)) ?? '').toUpperCase())
    .join('');
}

export type QualityInput = Pick<Track, 'format'> &
  Partial<Pick<Track, 'source' | 'bitDepth' | 'sampleRate' | 'bitrateKbps'>>;

const LOSSLESS = new Set(['flac', 'wav', 'aiff', 'aif', 'alac', 'ape', 'wv', 'dsf', 'dff']);
const NAMES: Record<string, string> = { aif: 'AIFF', oga: 'OGG', wv: 'WAVPACK' };

/** 96000 -> "96", 44100 -> "44.1". */
function khz(hz: number): string {
  return String(Math.round(hz / 100) / 10);
}

function isLossless(format: string, bitDepth: number | null | undefined): boolean {
  // m4a holds either ALAC (which reports a bit depth) or AAC.
  return LOSSLESS.has(format) || (format === 'm4a' && !!bitDepth);
}

/** The file's own label: "FLAC 24/96", "MP3 320", "OPUS". The format is the file extension. */
function fileLabel(track: QualityInput): string {
  const format = track.format.toLowerCase();
  const { bitDepth, sampleRate, bitrateKbps } = track;
  const lossless = isLossless(format, bitDepth);
  const name = format === 'm4a' ? (lossless ? 'ALAC' : 'AAC') : (NAMES[format] ?? format.toUpperCase());

  if (lossless) {
    return bitDepth && sampleRate ? `${name} ${bitDepth}/${khz(sampleRate)}` : name;
  }
  return bitrateKbps ? `${name} ${Math.round(bitrateKbps)}` : name;
}

/** The MP3 bitrate a Navidrome track streams at under `stream`, or null when it streams the
 *  file as it is. Navidrome sends the raw file only when it already is an MP3 at or below the
 *  cap; everything else (lossless, another codec, a higher or unknown bitrate) is transcoded
 *  to MP3 at the cap. Local tracks never transcode. */
function transcodeKbps(track: QualityInput, stream: StreamQuality | undefined): number | null {
  if (track.source !== 'navidrome' || !stream || stream === 'original') return null;
  const cap = Number(stream);
  const { bitrateKbps } = track;
  const passthrough = track.format.toLowerCase() === 'mp3' && !!bitrateKbps && bitrateKbps <= cap;
  return passthrough ? null : cap;
}

/** "FLAC 24/96", "MP3 320", "OPUS": how the track sounds as it plays. A Navidrome track that
 *  `stream` transcodes shows the transcode ("MP3 192"). */
export function qualityLabel(track: QualityInput, stream?: StreamQuality): string {
  const kbps = transcodeKbps(track, stream);
  return kbps ? `MP3 ${kbps}` : fileLabel(track);
}

/** The file's label behind a transcode ("FLAC 24/96" streamed as MP3 192), or null when the
 *  track plays as it is or nothing is known about the file. */
export function streamedFrom(track: QualityInput, stream?: StreamQuality): string | null {
  if (!transcodeKbps(track, stream) || !track.format) return null;
  return fileLabel(track);
}

/** "Local files · FLAC 24/96": where a track comes from and how it plays (see `qualityLabel`).
 *  A track whose format is unknown, and that is not transcoded, shows the source name alone. */
export function sourceQualityLabel(track: Track, sourceName: (s: Source) => string, stream?: StreamQuality): string {
  const source = sourceName(track.source);
  const quality = track.format || transcodeKbps(track, stream) ? qualityLabel(track, stream) : '';
  return quality ? `${source} · ${quality}` : source;
}
