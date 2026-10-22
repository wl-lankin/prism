// Writes src/lib/licenses.json, the list behind "Open-source licenses" in the About dialog.
// Run with `npm run licenses` after changing dependencies. The output is sorted and has no
// timestamps, so a rerun on unchanged dependencies changes nothing.
//
//   rust:    crates in the normal (non-dev, non-build) dependency graph of the prism package
//   npm:     production dependencies from package.json plus the dev dependencies that Vite
//            bundles into the shipped frontend (Svelte's runtime), with their transitive
//            dependencies, from package-lock.json
//   bundled: programs and data that are not packages, listed here by hand
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
const REPO = 'https://github.com/wl-lankin/prism';

// Dev dependencies whose code ends up in the shipped bundle.
const BUNDLED_DEV = ['svelte'];

const byName = (a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : a.version < b.version ? -1 : a.version > b.version ? 1 : 0);

function cleanUrl(value) {
  const raw = typeof value === 'string' ? value : value?.url;
  if (!raw) return '';
  return raw
    .replace(/^git\+/, '')
    .replace(/^git:\/\//, 'https://')
    .replace(/^ssh:\/\/git@github\.com\//, 'https://github.com/')
    .replace(/^github:/, 'https://github.com/')
    .replace(/\.git$/, '');
}

// Prism ships for these targets only; Linux-only crates (GTK, Wayland) are not in the installers.
const TARGETS = ['x86_64-pc-windows-msvc', 'aarch64-apple-darwin', 'x86_64-apple-darwin'];

function rustCrates() {
  const found = new Map();
  for (const target of TARGETS) {
    for (const crate of rustCratesFor(target)) found.set(`${crate.name}@${crate.version}`, crate);
  }
  return [...found.values()].sort(byName);
}

function rustCratesFor(target) {
  const out = execFileSync(
    'cargo',
    ['metadata', '--format-version', '1', '--locked', '--filter-platform', target, '--manifest-path', join(root, 'src-tauri/Cargo.toml')],
    { encoding: 'utf8', maxBuffer: 256 * 1024 * 1024 },
  );
  const meta = JSON.parse(out);
  const packages = new Map(meta.packages.map((p) => [p.id, p]));
  const nodes = new Map(meta.resolve.nodes.map((n) => [n.id, n]));
  const rootId = meta.resolve.root ?? meta.packages.find((p) => p.name === 'prism')?.id;
  if (!rootId) throw new Error('the prism package is not in the cargo metadata');

  // Follow only normal edges: a dev or build dependency never ships in the binary.
  const seen = new Set();
  const stack = [rootId];
  while (stack.length > 0) {
    const id = stack.pop();
    if (seen.has(id)) continue;
    seen.add(id);
    for (const dep of nodes.get(id).deps) {
      if (dep.dep_kinds.some((k) => k.kind === null)) stack.push(dep.pkg);
    }
  }
  seen.delete(rootId);

  return [...seen]
    .map((id) => packages.get(id))
    .map((p) => ({
      name: p.name,
      version: p.version,
      license: p.license ?? (p.license_file ? `see ${p.license_file}` : ''),
      repository: cleanUrl(p.repository),
    }));
}

function npmPackages() {
  const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
  const lock = JSON.parse(readFileSync(join(root, 'package-lock.json'), 'utf8'));
  const packages = lock.packages;

  // Node resolution inside the lockfile: the nearest node_modules/<name> up the path.
  const resolve = (from, name) => {
    let base = from;
    for (;;) {
      const key = `${base}${base ? '/' : ''}node_modules/${name}`;
      if (packages[key]) return key;
      if (!base) return null;
      const cut = base.lastIndexOf('/node_modules/');
      base = cut === -1 ? '' : base.slice(0, cut);
    }
  };

  const seen = new Set();
  const stack = [...Object.keys(pkg.dependencies ?? {}), ...BUNDLED_DEV].map((name) => resolve('', name));
  if (stack.includes(null)) throw new Error('a dependency from package.json is missing in package-lock.json');
  while (stack.length > 0) {
    const key = stack.pop();
    if (seen.has(key)) continue;
    seen.add(key);
    for (const name of Object.keys(packages[key].dependencies ?? {})) {
      const dep = resolve(key, name);
      if (dep) stack.push(dep);
    }
  }

  return [...seen]
    .map((key) => {
      const entry = packages[key];
      const name = key.slice(key.lastIndexOf('node_modules/') + 'node_modules/'.length);
      const manifestPath = join(root, key, 'package.json');
      const manifest = existsSync(manifestPath) ? JSON.parse(readFileSync(manifestPath, 'utf8')) : {};
      const license = entry.license ?? (typeof manifest.license === 'string' ? manifest.license : manifest.license?.type) ?? '';
      return { name, version: entry.version, license, repository: cleanUrl(manifest.repository) };
    })
    .sort(byName);
}

function ffmpegVersion() {
  const config = readFileSync(join(root, 'scripts/ffmpeg-config.sh'), 'utf8');
  const match = config.match(/^FFMPEG_VERSION="([^"]+)"/m);
  if (!match) throw new Error('FFMPEG_VERSION not found in scripts/ffmpeg-config.sh');
  return match[1];
}

function bundled() {
  const ffmpeg = ffmpegVersion();
  return [
    {
      // The notes for these four are in i18n.ts (about.note.<id>).
      id: 'ffmpeg',
      name: 'FFmpeg',
      version: ffmpeg,
      license: 'LGPL-2.1-or-later',
      repository: `${REPO}/blob/main/docs/ffmpeg.md`,
      source: `https://ffmpeg.org/releases/ffmpeg-${ffmpeg}.tar.xz`,
    },
    {
      id: 'geojs',
      name: 'GeoJS',
      version: '',
      license: 'Free service',
      repository: 'https://www.geojs.io',
    },
    {
      id: 'navidrome',
      name: 'Navidrome logo',
      version: '',
      license: 'GPL-3.0',
      repository: 'https://github.com/navidrome/navidrome',
    },
    {
      id: 'radio-browser',
      name: 'Radio Browser',
      version: '',
      license: 'Free community API',
      repository: 'https://www.radio-browser.info',
    },
  ].sort(byName);
}

const data = { rust: rustCrates(), npm: npmPackages(), bundled: bundled() };
writeFileSync(join(root, 'src/lib/licenses.json'), `${JSON.stringify(data, null, 2)}\n`);
console.log(`rust ${data.rust.length}, npm ${data.npm.length}, bundled ${data.bundled.length}`);
