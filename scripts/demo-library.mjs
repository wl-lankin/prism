// A synthetic demo library: fictional artists and albums, generated covers, silent FLAC.
// Used by `node scripts/make-fixtures.mjs --demo <n>`. Deterministic for a given <n>.
//
// ffmpeg encodes a handful of silent base files once (one per duration and format); every
// track is then assembled in-process from a base file's audio frames plus its own Vorbis
// comment block, so thousands of tracks take seconds. Covers are SVG rendered with resvg and
// stored as `cover.png` next to the tracks.
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Resvg } from '@resvg/resvg-js';

/** Seconds: a spread of ordinary song lengths. */
const DURATIONS = [148, 171, 189, 203, 217, 232, 246, 263, 281, 304, 337, 372];
/** [bits, sample rate]: most albums are CD quality, some hi-res. */
const FORMATS = [
  [16, 44100],
  [24, 96000],
];

const FIRST = [
  'Velvet', 'Neon', 'Paper', 'Silver', 'Hollow', 'Amber', 'Glass', 'Northern', 'Quiet', 'Electric',
  'Saltwater', 'Midnight', 'Copper', 'Lunar', 'Wild', 'Static', 'Golden', 'Distant', 'Crimson', 'Pale',
  'Low', 'Wandering', 'Iron', 'Soft', 'Cobalt', 'Feral', 'Tender', 'Hidden', 'Polar', 'Sundial',
];
const SECOND = [
  'Harbor', 'Orchard', 'Lanterns', 'Engines', 'Season', 'Pilots', 'Gardens', 'Cartographers', 'Tides', 'Choir',
  'Satellites', 'Atlas', 'Weather', 'Foxes', 'Monuments', 'Rivers', 'Radio', 'Lighthouse', 'Signals', 'Parade',
  'Archive', 'Meadows', 'Cinema', 'Arcade', 'Telegraph', 'Comets', 'Hotel', 'Assembly', 'Frontier', 'Society',
];
const SOLO = [
  'Mara Lindqvist', 'Theo Arkwright', 'Junie Okafor', 'Elias Brenner', 'Noor Halvorsen', 'Sasha Vell',
  'Ines Moreau', 'Kasimir Roth', 'Lotte Engström', 'Ravi Castell', 'Wren Abernathy', 'Odile Fanshawe',
  'Jonas Wiegand', 'Yara Delacroix', 'Felix Nordahl', 'Clara Sörensen', 'Mika Tanaka-Hall', 'Aurelio Bianchi',
];
const WORDS = [
  'Afterglow', 'Static', 'Harbor Lights', 'Slow Motion', 'Paper Moons', 'Undertow', 'Blue Hour', 'Signal Fire',
  'Open Water', 'Low Tide', 'Long Division', 'Night Swim', 'Small Hours', 'Weightless', 'Daybreak', 'Fault Lines',
  'Echo Park', 'Polaroid', 'Wildflower', 'Heat Mirage', 'Northbound', 'Telescope', 'Glasshouse', 'Snowblind',
  'Tin Roof', 'Overpass', 'Monsoon', 'Satellite Heart', 'Ferris Wheel', 'Quiet Riot', 'Lantern', 'Riverbed',
  'Copper Sky', 'Soft Focus', 'Cassette', 'Saltlines', 'Wanderlust', 'Gravity', 'Kaleidoscope', 'Driftwood',
  'Neon Rain', 'First Light', 'Carousel', 'Paper Planes', 'Halcyon', 'Starling', 'Undercurrent', 'Vespertine',
  'Ghost Town', 'Hourglass', 'Silver Lining', 'Pale Fire', 'Morning Static', 'Tidal', 'Afterimage', 'Wavelength',
];
const GENRES = ['Indie', 'Electronic', 'Jazz', 'Ambient', 'Rock', 'Folk', 'Soul', 'Pop', 'Classical', 'Hip-Hop'];

/** Two-color moods for the covers. */
const PALETTES = [
  ['#ff4d6d', '#2a1a5e'], ['#ffc94d', '#c2185b'], ['#4dffb2', '#0b3d4f'], ['#4da6ff', '#1b1446'],
  ['#f4a261', '#264653'], ['#e9c46a', '#2a9d8f'], ['#ff8fab', '#3a0ca3'], ['#90e0ef', '#03045e'],
  ['#fefae0', '#bc6c25'], ['#cdb4db', '#22223b'], ['#ffd6a5', '#9d0208'], ['#b9fbc0', '#3c096c'],
  ['#f1faee', '#e63946'], ['#ffbe0b', '#3a86ff'], ['#d8f3dc', '#1b4332'], ['#ffcad4', '#5f0f40'],
];

/** mulberry32: a tiny seeded PRNG so the same <n> always builds the same library. */
function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const safe = (name) => name.replace(/[<>:"/\\|?*]/g, '').trim();
const esc = (text) => text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

/** STREAMINFO and the audio frames of a FLAC file; every other metadata block is dropped. */
function flacParts(buf) {
  if (buf.toString('latin1', 0, 4) !== 'fLaC') throw new Error('not a FLAC file');
  let p = 4;
  let streaminfo;
  for (;;) {
    const header = buf[p];
    const length = buf.readUIntBE(p + 1, 3);
    if ((header & 0x7f) === 0) streaminfo = buf.subarray(p + 4, p + 4 + length);
    p += 4 + length;
    if (header & 0x80) break;
  }
  return { streaminfo, frames: buf.subarray(p) };
}

function metaBlock(type, data, last) {
  const header = Buffer.alloc(4);
  header[0] = (last ? 0x80 : 0) | type;
  header.writeUIntBE(data.length, 1, 3);
  return Buffer.concat([header, data]);
}

function u32le(n) {
  const b = Buffer.alloc(4);
  b.writeUInt32LE(n);
  return b;
}

function vorbisComment(tags) {
  const vendor = Buffer.from('Prism demo library');
  const entries = Object.entries(tags).map(([key, value]) => Buffer.from(`${key}=${value}`, 'utf8'));
  return Buffer.concat([u32le(vendor.length), vendor, u32le(entries.length), ...entries.flatMap((e) => [u32le(e.length), e])]);
}

function coverSvg(random, title, artist) {
  const [light, dark] = PALETTES[Math.floor(random() * PALETTES.length)];
  const motif = Math.floor(random() * 5);
  const angle = Math.floor(random() * 360);
  const cx = 150 + random() * 300;
  const cy = 120 + random() * 280;
  let art = '';
  if (motif === 0) {
    art = `<circle cx="${cx}" cy="${cy}" r="${120 + random() * 90}" fill="${light}" opacity="0.92"/>`;
  } else if (motif === 1) {
    for (let r = 40; r < 420; r += 34) art += `<circle cx="${cx}" cy="${cy}" r="${r}" fill="none" stroke="${light}" stroke-width="2" opacity="${(1 - r / 440).toFixed(2)}"/>`;
  } else if (motif === 2) {
    for (let i = 0; i < 7; i++) art += `<rect x="-200" y="${i * 120 - 100}" width="1000" height="${26 + i * 6}" fill="${light}" opacity="${(0.15 + i * 0.1).toFixed(2)}" transform="rotate(${angle % 60 - 30} 300 300)"/>`;
  } else if (motif === 3) {
    for (let x = 60; x < 600; x += 48) for (let y = 60; y < 460; y += 48) art += `<circle cx="${x}" cy="${y}" r="${3 + ((x * y) % 7)}" fill="${light}" opacity="0.75"/>`;
  } else {
    art = `<path d="M0 ${380 + random() * 80} Q 150 ${200 + random() * 120} 300 ${330 + random() * 60} T 600 ${260 + random() * 100} V 600 H 0 Z" fill="${light}" opacity="0.9"/>`;
  }
  return `<svg xmlns="http://www.w3.org/2000/svg" width="600" height="600" viewBox="0 0 600 600">
  <defs>
    <linearGradient id="bg" gradientTransform="rotate(${angle} 0.5 0.5)">
      <stop offset="0" stop-color="${dark}"/>
      <stop offset="1" stop-color="${light}" stop-opacity="0.55"/>
    </linearGradient>
    <radialGradient id="shade" cx="0.5" cy="0.4" r="0.8">
      <stop offset="0.5" stop-color="#000" stop-opacity="0"/>
      <stop offset="1" stop-color="#000" stop-opacity="0.45"/>
    </radialGradient>
  </defs>
  <rect width="600" height="600" fill="${dark}"/>
  <rect width="600" height="600" fill="url(#bg)"/>
  ${art}
  <rect width="600" height="600" fill="url(#shade)"/>
  <text x="40" y="540" font-family="Segoe UI, Helvetica, Arial, sans-serif" font-size="34" font-weight="700" fill="#ffffff">${esc(title)}</text>
  <text x="40" y="568" font-family="Segoe UI, Helvetica, Arial, sans-serif" font-size="20" fill="#ffffff" fill-opacity="0.75">${esc(artist)}</text>
</svg>`;
}

export async function makeDemo(dir, total) {
  const random = rng(total * 7919 + 17);
  const pick = (list) => list[Math.floor(random() * list.length)];
  const work = mkdtempSync(join(tmpdir(), 'prism-demo-'));
  try {
    // Silent base files: one per duration and format.
    const bases = new Map();
    for (const [bits, rate] of FORMATS) {
      for (const seconds of DURATIONS) {
        const file = join(work, `${bits}-${rate}-${seconds}.flac`);
        execFileSync('ffmpeg', [
          '-hide_banner', '-loglevel', 'error', '-y',
          '-f', 'lavfi', '-i', `anullsrc=r=${rate}:cl=stereo`, '-t', String(seconds),
          '-c:a', 'flac', '-sample_fmt', bits === 16 ? 's16' : 's32', '-bits_per_raw_sample', String(bits),
          '-frame_size', '65535', file,
        ]);
        bases.set(`${bits}-${rate}-${seconds}`, flacParts(readFileSync(file)));
      }
    }

    // Artists: bands and solo names, about three albums each.
    const albumCount = Math.max(1, Math.round(total / 10));
    const artistCount = Math.max(1, Math.ceil(albumCount / 3));
    const artists = new Set();
    while (artists.size < artistCount) {
      artists.add(random() < 0.3 ? pick(SOLO) : `${pick(FIRST)} ${pick(SECOND)}`);
      if (artists.size >= FIRST.length * SECOND.length) break;
    }
    const artistList = [...artists];

    rmSync(dir, { recursive: true, force: true });
    mkdirSync(dir, { recursive: true });

    let written = 0;
    const albumTitles = new Set();
    for (let a = 0; a < albumCount && written < total; a++) {
      const artist = artistList[a % artistList.length];
      let title;
      do title = random() < 0.5 ? pick(WORDS) : `${pick(WORDS)} ${pick(['Sessions', 'Vol. 2', 'Live', 'Revisited', 'Suite', 'Nocturnes'])}`;
      while (albumTitles.has(`${artist}|${title}`) && albumTitles.size < 100000);
      albumTitles.add(`${artist}|${title}`);
      const year = 1968 + Math.floor(random() * 58);
      const genre = pick(GENRES);
      const [bits, rate] = random() < 0.15 ? FORMATS[1] : FORMATS[0];
      const left = total - written;
      const remainingAlbums = albumCount - a;
      // About ten per album, spread so the last album still gets a normal share.
      const share = Math.round(left / remainingAlbums) + Math.floor(random() * 5) - 2;
      const size = remainingAlbums === 1 ? left : Math.max(1, Math.min(left - (remainingAlbums - 1), share));

      const folder = join(dir, safe(artist), `${year} ${safe(title)}`);
      mkdirSync(folder, { recursive: true });
      const png = new Resvg(coverSvg(random, title, artist), {
        fitTo: { mode: 'width', value: 600 },
        font: { loadSystemFonts: true, defaultFontFamily: 'Arial' },
      })
        .render()
        .asPng();
      writeFileSync(join(folder, 'cover.png'), png);

      for (let n = 1; n <= size; n++) {
        const name = pick(WORDS);
        const seconds = pick(DURATIONS);
        const base = bases.get(`${bits}-${rate}-${seconds}`);
        const tags = {
          TITLE: name,
          ARTIST: artist,
          ALBUMARTIST: artist,
          ALBUM: title,
          TRACKNUMBER: String(n),
          TRACKTOTAL: String(size),
          DISCNUMBER: '1',
          DATE: String(year),
          GENRE: genre,
        };
        const file = Buffer.concat([
          Buffer.from('fLaC', 'latin1'),
          metaBlock(0, base.streaminfo, false),
          metaBlock(4, vorbisComment(tags), true),
          base.frames,
        ]);
        writeFileSync(join(folder, `${String(n).padStart(2, '0')} ${safe(name)}.flac`), file);
        written++;
      }
    }
    console.log(`demo library: ${written} tracks in ${albumCount} albums by ${artistList.length} artists at ${dir}`);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}
