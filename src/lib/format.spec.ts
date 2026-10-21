import { describe, expect, it } from 'vitest';
import { formatDuration, qualityLabel, sourceQualityLabel, streamedFrom } from './format';
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
  const names: Record<Source, string> = { local: 'Local files', navidrome: 'Navidrome', radio: 'Radio' };
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

  it('shows what a Navidrome transcode streams', () => {
    const nd = track({ source: 'navidrome', bitDepth: 24, sampleRate: 96000 });
    expect(sourceQualityLabel(nd, name, '192')).toBe('Navidrome · MP3 192');
    expect(sourceQualityLabel(nd, name, 'original')).toBe('Navidrome · FLAC 24/96');
    // The format is unknown, but a transcode is MP3 at the cap.
    expect(sourceQualityLabel(track({ source: 'navidrome', format: '' }), name, '128')).toBe('Navidrome · MP3 128');
  });
});

describe('streamed quality', () => {
  const nd = (over: Partial<Track>): Track =>
    ({ source: 'navidrome', format: 'flac', bitDepth: null, sampleRate: null, bitrateKbps: null, ...over }) as Track;

  it('shows the MP3 cap for a lossless Navidrome track', () => {
    expect(qualityLabel(nd({ bitDepth: 24, sampleRate: 96000 }), '192')).toBe('MP3 192');
  });

  it('keeps the original label only for an MP3 at or below the cap', () => {
    expect(qualityLabel(nd({ format: 'mp3', bitrateKbps: 128 }), '320')).toBe('MP3 128');
    expect(qualityLabel(nd({ format: 'mp3', bitrateKbps: 320 }), '320')).toBe('MP3 320');
  });

  it('shows the cap for another lossy codec, even below the cap', () => {
    const aac = nd({ format: 'm4a', bitrateKbps: 256 });
    expect(qualityLabel(aac, '320')).toBe('MP3 320');
    expect(streamedFrom(aac, '320')).toBe('AAC 256');
    expect(qualityLabel(nd({ format: 'ogg', bitrateKbps: 96 }), '128')).toBe('MP3 128');
  });

  it('shows the cap when the bitrate is above it or unknown', () => {
    expect(qualityLabel(nd({ format: 'mp3', bitrateKbps: 320 }), '128')).toBe('MP3 128');
    expect(qualityLabel(nd({ format: 'ogg' }), '192')).toBe('MP3 192');
  });

  it('leaves local tracks and the original stream unchanged', () => {
    const local = { ...nd({ bitDepth: 24, sampleRate: 96000 }), source: 'local' } as Track;
    expect(qualityLabel(local, '128')).toBe('FLAC 24/96');
    expect(qualityLabel(nd({ bitDepth: 24, sampleRate: 96000 }), 'original')).toBe('FLAC 24/96');
    expect(qualityLabel(nd({ bitDepth: 24, sampleRate: 96000 }))).toBe('FLAC 24/96');
  });

  it('names the file behind a transcode, and nothing otherwise', () => {
    expect(streamedFrom(nd({ bitDepth: 24, sampleRate: 96000 }), '192')).toBe('FLAC 24/96');
    expect(streamedFrom(nd({ format: 'mp3', bitrateKbps: 128 }), '320')).toBeNull();
    expect(streamedFrom(nd({ bitDepth: 24, sampleRate: 96000 }), 'original')).toBeNull();
    // Unknown file details: nothing worth naming.
    expect(streamedFrom(nd({ format: '' }), '128')).toBeNull();
  });
});
