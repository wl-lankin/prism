// Generates the small audio fixtures the Rust library tests read.
// Run with `node scripts/make-fixtures.mjs`; needs `ffmpeg` on PATH.
// Every file is a 1-second sine (or derived from one), so the set stays far below 300 KB.
//
// `node scripts/make-fixtures.mjs --radio` writes only the generated radio fixtures.
// `node scripts/make-fixtures.mjs --radio-streams` writes only the HLS segments and the MKA tone.
//
// `node scripts/make-fixtures.mjs --demo <n>` instead writes a synthetic demo library of
// <n> tagged FLAC tracks (about 10 per album, fictional names, generated cover art, silent
// audio) to the gitignored `demo/` folder, for trying the library views at scale.
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
const out = join(root, 'src-tauri/tests/fixtures');

const demoFlag = process.argv.indexOf('--demo');
if (demoFlag !== -1) {
  const count = Number(process.argv[demoFlag + 1] ?? 3000);
  if (!Number.isInteger(count) || count < 1) {
    console.error('usage: node scripts/make-fixtures.mjs --demo <number of tracks>');
    process.exit(1);
  }
  const { makeDemo } = await import('./demo-library.mjs');
  await makeDemo(join(root, 'demo'), count);
  process.exit(0);
}

const work = mkdtempSync(join(tmpdir(), 'prism-fixtures-'));

const ffmpeg = (args) =>
  execFileSync('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-y', ...args], { stdio: 'inherit' });

const hasEncoder = (name) => {
  const list = execFileSync('ffmpeg', ['-hide_banner', '-encoders'], { encoding: 'utf8' });
  return list.split('\n').some((line) => line.trim().split(/\s+/)[1] === name);
};

// Keeps ffmpeg from writing its own "encoder" tags so untagged files really have no tags.
const bitexact = ['-fflags', '+bitexact', '-flags:a', '+bitexact', '-map_metadata', '-1'];
const sine = ['-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=22050:duration=1'];
const mono = ['-ac', '1'];

const meta = (tags) => Object.entries(tags).flatMap(([key, value]) => ['-metadata', `${key}=${value}`]);

/** ID3v2.3 tag with a single TIT2 frame encoded as ISO-8859-1 (encoding byte 0x00). */
const id3v23Latin1Title = (title) => {
  const text = Buffer.concat([Buffer.from([0x00]), Buffer.from(title, 'latin1')]);
  const frameHeader = Buffer.alloc(10);
  frameHeader.write('TIT2', 0, 'ascii');
  frameHeader.writeUInt32BE(text.length, 4);
  const frames = Buffer.concat([frameHeader, text]);
  const size = frames.length;
  const header = Buffer.from([
    0x49, 0x44, 0x33, 0x03, 0x00, 0x00,
    (size >> 21) & 0x7f, (size >> 14) & 0x7f, (size >> 7) & 0x7f, size & 0x7f,
  ]);
  return Buffer.concat([header, frames]);
};

// The fake radio station's endless body: CBR MP3 frames without ID3 tags or a Xing header, so
// the file loops cleanly when it is served back to back.
const makeRadio = () => {
  mkdirSync(join(out, 'radio'), { recursive: true });
  ffmpeg([
    '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=44100:duration=2', '-ac', '2', ...bitexact,
    '-c:a', 'libmp3lame', '-b:a', '128k', '-id3v2_version', '0', '-write_id3v1', '0', '-write_xing', '0',
    join(out, 'radio/tone.mp3'),
  ]);
};

// The radio opener's bodies: two short HLS segments (AAC in MPEG-TS, in sequence) for the fake
// HLS station, and an Opus-in-Matroska tone, which symphonia cannot decode (the FFmpeg fallback).
const makeRadioStreams = () => {
  mkdirSync(join(out, 'radio'), { recursive: true });
  const hls = join(work, 'hls');
  mkdirSync(hls, { recursive: true });
  ffmpeg([
    '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=22050:duration=4', ...mono, ...bitexact,
    '-c:a', 'aac', '-b:a', '32k', '-f', 'hls', '-hls_time', '2', '-hls_list_size', '0',
    '-start_number', '1', '-hls_segment_filename', join(hls, 'seg%d.ts'), join(hls, 'list.m3u8'),
  ]);
  for (const name of ['seg1.ts', 'seg2.ts']) {
    writeFileSync(join(out, 'radio', name), readFileSync(join(hls, name)));
  }
  const opusArgs = hasEncoder('libopus') ? ['-c:a', 'libopus'] : ['-c:a', 'opus', '-strict', '-2'];
  ffmpeg([
    '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000:duration=2', ...mono, ...bitexact,
    ...opusArgs, '-b:a', '24k', '-f', 'matroska', join(out, 'radio/tone.mka'),
  ]);
  // ADTS AAC, the way AAC stations send it (FFmpeg decodes it: symphonia's AAC is LC only).
  ffmpeg([
    '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=22050:duration=2', ...mono, ...bitexact,
    '-c:a', 'aac', '-b:a', '32k', '-f', 'adts', join(out, 'radio/tone.aac'),
  ]);
  // A chained Ogg: two 2 s Vorbis streams back to back, the second at another rate, like a
  // station that starts a new chain link for the next song. Stereo: FFmpeg's own Vorbis encoder
  // (used when libvorbis is missing) has no mono, and writes silence at `-q:a 0`.
  const vorbis = hasEncoder('libvorbis')
    ? ['-c:a', 'libvorbis', '-q:a', '0']
    : ['-c:a', 'vorbis', '-strict', '-2'];
  const link = (name, rate, seconds) => {
    const path = join(work, name);
    ffmpeg([
      '-f', 'lavfi', '-i', `sine=frequency=440:sample_rate=${rate}:duration=${seconds}`,
      '-ac', '2', ...bitexact, '-metadata:s:a:0', 'encoder=', ...vorbis, path,
    ]);
    return readFileSync(path);
  };
  writeFileSync(
    join(out, 'radio/chain.ogg'),
    Buffer.concat([link('a.ogg', 22050, 2), link('b.ogg', 44100, 2)]),
  );
  // One 0.2 s Vorbis link: served back to back, every link ends before a decoder may rechain.
  writeFileSync(join(out, 'radio/short.ogg'), link('short.ogg', 22050, 0.2));
};

// Hand-written fixtures (Subsonic and Radio Browser JSON) live next to the generated files.
const handWritten = new Set(['subsonic', 'radio']);

if (process.argv.includes('--radio-streams')) {
  try {
    makeRadioStreams();
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
  process.exit(0);
}

if (process.argv.includes('--radio')) {
  try {
    makeRadio();
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
  process.exit(0);
}

try {
  mkdirSync(out, { recursive: true });
  for (const entry of readdirSync(out)) {
    if (!handWritten.has(entry)) rmSync(join(out, entry), { recursive: true, force: true });
  }
  mkdirSync(join(out, 'cover'), { recursive: true });
  makeRadio();
  makeRadioStreams();

  const png = join(work, 'cover.png');
  ffmpeg(['-f', 'lavfi', '-i', 'color=c=0xff4d6d:s=64x64', '-frames:v', '1', png]);

  ffmpeg([
    ...sine, ...mono, ...bitexact, '-c:a', 'libmp3lame', '-b:a', '32k', '-id3v2_version', '3',
    ...meta({ title: 'Alpha', artist: 'Fixture Artist', album_artist: 'Fixture Artist', album: 'Fixture Album', track: '1' }),
    join(out, 'tagged.mp3'),
  ]);

  ffmpeg([
    ...sine, '-i', png, '-map', '0:a', '-map', '1:v', ...mono, ...bitexact,
    '-c:a', 'flac', '-c:v', 'png', '-disposition:v', 'attached_pic',
    '-metadata:s:v', 'comment=Cover (front)',
    ...meta({
      title: 'Beta', artist: 'Fixture Artist', album_artist: 'Fixture Artist', album: 'Fixture Album', track: '2',
      REPLAYGAIN_TRACK_GAIN: '-6.50 dB', REPLAYGAIN_ALBUM_GAIN: '-7.25 dB',
      REPLAYGAIN_TRACK_PEAK: '0.891250', REPLAYGAIN_ALBUM_PEAK: '0.977000',
    }),
    join(out, 'tagged.flac'),
  ]);

  ffmpeg([
    ...sine, ...mono, ...bitexact, '-metadata:s:a:0', 'encoder=', '-c:a', 'libvorbis', '-q:a', '0',
    join(out, 'notags.ogg'),
  ]);

  // ffmpeg writes non-ASCII ID3v2.3 text as UTF-16, so the Latin-1 tag is built by hand.
  const plainMp3 = join(work, 'plain.mp3');
  ffmpeg([...sine, ...mono, ...bitexact, '-c:a', 'libmp3lame', '-b:a', '32k', '-id3v2_version', '0', '-write_id3v1', '0', plainMp3]);
  writeFileSync(join(out, 'latin1.mp3'), Buffer.concat([id3v23Latin1Title('Café'), readFileSync(plainMp3)]));

  const opusArgs = hasEncoder('libopus') ? ['-c:a', 'libopus'] : ['-c:a', 'opus', '-strict', '-2'];
  if (!hasEncoder('libopus')) console.warn('libopus missing: using the native ffmpeg opus encoder');
  ffmpeg([...sine, ...mono, ...bitexact, ...opusArgs, '-b:a', '24k', ...meta({ title: 'Opus' }), join(out, 'opus.opus')]);
  ffmpeg([...sine, ...mono, ...bitexact, '-c:a', 'wmav2', '-b:a', '32k',
    ...meta({ title: 'Wma', artist: 'Fixture Artist', album: 'Probe Album', track: '3', date: '2024' }), join(out, 'wma.wma')]);
  ffmpeg([...sine, ...mono, ...bitexact, '-c:a', 'wavpack', ...meta({ title: 'WavPack' }), join(out, 'wv.wv')]);
  ffmpeg([...sine, ...mono, ...bitexact, '-c:a', 'tta', join(out, 'tta.tta')]);
  ffmpeg([...sine, ...mono, ...bitexact, '-c:a', 'aac', '-b:a', '32k', ...meta({ title: 'Aac' }), join(out, 'aac.m4a')]);

  // Two PCM files with the same spec (44.1 kHz stereo 16-bit) for decoding and gapless tests.
  for (const [name, frequency] of [['gap_a.wav', 440], ['gap_b.wav', 660]]) {
    ffmpeg([
      '-f', 'lavfi', '-i', `sine=frequency=${frequency}:sample_rate=44100:duration=1`,
      '-ac', '2', ...bitexact, '-c:a', 'pcm_s16le', join(out, name),
    ]);
  }

  // A valid FLAC header followed by zeros: readable magic, unusable stream.
  const validFlac = join(work, 'valid.flac');
  ffmpeg([...sine, ...mono, ...bitexact, '-c:a', 'flac', validFlac]);
  const corrupt = Buffer.alloc(4096);
  readFileSync(validFlac).copy(corrupt, 0, 0, 200);
  writeFileSync(join(out, 'corrupt.flac'), corrupt);

  ffmpeg(['-f', 'lavfi', '-i', 'color=c=0x4da6ff:s=64x64', '-frames:v', '1', '-q:v', '5', join(out, 'cover/folder.jpg')]);
  ffmpeg([
    ...sine, ...mono, ...bitexact, '-c:a', 'libmp3lame', '-b:a', '32k', '-id3v2_version', '3',
    ...meta({ title: 'Gamma', artist: 'Cover Artist', album: 'Cover Album', track: '1' }),
    join(out, 'cover/track.mp3'),
  ]);
} finally {
  rmSync(work, { recursive: true, force: true });
}
