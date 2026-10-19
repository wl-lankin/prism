// Renders the brand SVG and lets the Tauri CLI build every app icon size from it.
// Run with `npm run icons` after changing assets/logo.svg.
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Resvg } from '@resvg/resvg-js';

const root = fileURLToPath(new URL('..', import.meta.url));
const render = (svg, size) =>
  new Resvg(readFileSync(join(root, svg)), { fitTo: { mode: 'width', value: size } }).render().asPng();

// Runs the local Tauri CLI through node directly: no shell, so no quoting or npx.cmd lookup on Windows.
const cli = join(root, 'node_modules/@tauri-apps/cli/tauri.js');
const tauri = (args) => execFileSync(process.execPath, [cli, ...args], { cwd: root, stdio: 'inherit' });

const work = mkdtempSync(join(tmpdir(), 'prism-icons-'));
try {
  const logo = join(work, 'logo.png');
  writeFileSync(logo, render('assets/logo.svg', 1024));
  tauri(['icon', logo, '-o', 'src-tauri/icons']);

  // Prism ships for Windows and macOS only, so drop the mobile icon sets the CLI also writes.
  rmSync(join(root, 'src-tauri/icons/android'), { recursive: true, force: true });
  rmSync(join(root, 'src-tauri/icons/ios'), { recursive: true, force: true });

  // Brand image for the README and the About dialog.
  mkdirSync(join(root, 'docs'), { recursive: true });
  writeFileSync(join(root, 'docs/logo.png'), render('assets/logo.svg', 512));
} finally {
  rmSync(work, { recursive: true, force: true });
}
