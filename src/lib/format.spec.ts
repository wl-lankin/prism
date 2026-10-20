import { describe, expect, it } from 'vitest';
import { formatDuration, qualityLabel, sourceQualityLabel } from './format';
import type { Source, Track } from './types';

describe('formatDuration', () => {
  it('formats minutes and seconds', () => {
    expect(formatDuration(187000)).toBe('3:07');
  });

  it('formats hours', () => {
    expect(formatDuration(3723000)).toBe('1:02:03');
  });

  it('handles zero, sub-second and invalid values', () => {
    expect(formatDuration(0)).toBe('0:00');
    expect(formatDuration(999)).toBe('0:00');
    expect(formatDuration(-5)).toBe('0:00');
    expect(formatDuration(Number.NaN)).toBe('0:00');
  });
});

describe('qualityLabel', () => {
  it('shows bit depth and sample rate for lossless files', () => {
    expect(qualityLabel({ format: 'flac', bitDepth: 24, sampleRate: 96000 })).toBe('FLAC 24/96');
    expect(qualityLabel({ format: 'flac', bitDepth: 16, sampleRate: 44100 })).toBe('FLAC 16/44.1');
  });

  it('shows the bitrate for lossy files', () => {
    expect(qualityLabel({ format: 'mp3', bitrateKbps: 320 })).toBe('MP3 320');
  });

  it('shows only the format when details are missing', () => {
    expect(qualityLabel({ format: 'opus' })).toBe('OPUS');
    expect(qualityLabel({ format: 'flac', bitDepth: null, sampleRate: null })).toBe('FLAC');
  });

  it('names m4a by its codec', () => {
    expect(qualityLabel({ format: 'm4a', bitDepth: 16, sampleRate: 44100 })).toBe('ALAC 16/44.1');
    expect(qualityLabel({ format: 'm4a', bitrateKbps: 256 })).toBe('AAC 256');
  });
});

describe('sourceQualityLabel', () => {
  const names: Record<Source, string> = { local: 'Local files', navidrome: 'Navidrome' };
  const name = (s: Source) => names[s];
  const base = { source: 'local', format: 'flac', bitDepth: null, sampleRate: null, bitrateKbps: null } as const;
  const track = (over: Partial<Track>) => ({ ...base, ...over }) as Track;

  it('joins the source and the quality', () => {
    expect(sourceQualityLabel(track({ bitDepth: 24, sampleRate: 96000 }), name)).toBe('Local files · FLAC 24/96');
    expect(sourceQualityLabel(track({ source: 'navidrome', format: 'mp3', bitrateKbps: 320 }), name)).toBe(
      'Navidrome · MP3 320',
    );
  });

  it('shows only the source name for a track without quality', () => {
    expect(sourceQualityLabel(track({ source: 'navidrome', format: '' }), name)).toBe('Navidrome');
  });
});
