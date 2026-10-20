// Renders the brand SVG and lets the Tauri CLI build every app icon size from it.
// Run with `npm run icons` after changing assets/logo.svg.
// `npm run icons -- --shell` only writes the shell icons: the macOS menu bar template and the
// Windows thumbnail toolbar buttons.
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Resvg } from '@resvg/resvg-js';

const root = fileURLToPath(new URL('..', import.meta.url));
const renderSvg = (svg, size) => new Resvg(svg, { fitTo: { mode: 'width', value: size } }).render().asPng();
const render = (file, size) => renderSvg(readFileSync(join(root, file)), size);

// The prism and its incoming beam from the logo, as a black silhouette with alpha: macOS
// draws a template image in the menu bar's own color.
const TRAY_TEMPLATE = `<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="60 180 900 640">
  <g fill="none" stroke="#000" stroke-linecap="round" stroke-linejoin="round" stroke-width="72">
    <path d="M566 456 Q 760 446 900 354"/>
    <path d="M614 540 Q 770 540 918 540"/>
    <path d="M662 624 Q 790 634 900 726"/>
    <path d="M116 540 L286 540"/>
    <path d="M450 256 L739 756 L161 756 Z"/>
  </g>
</svg>`;

// The thumbnail toolbar buttons: the transport glyphs of Icon.svelte, filled, in the text color.
const glyph = (paths) => `<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24">
  <g fill="#f3f3f5" stroke="#f3f3f5" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">${paths}</g>
</svg>`;
const THUMBS = {
  prev: '<path d="M6.5 5.5v13"/><path d="M18.5 6.6v10.8a.9.9 0 0 1-1.4.7L9.6 12.7a.9.9 0 0 1 0-1.4l7.5-5.4a.9.9 0 0 1 1.4.7z"/>',
  play: '<path d="M7.5 5.6v12.8a.9.9 0 0 0 1.4.8l10-6.4a.9.9 0 0 0 0-1.6l-10-6.4a.9.9 0 0 0-1.4.8z"/>',
  pause: '<rect x="6.5" y="5.5" width="3.5" height="13" rx="1"/><rect x="14" y="5.5" width="3.5" height="13" rx="1"/>',
  next: '<path d="M17.5 5.5v13"/><path d="M5.5 6.6v10.8a.9.9 0 0 0 1.4.7l7.5-5.4a.9.9 0 0 0 0-1.4L6.9 5.9a.9.9 0 0 0-1.4.7z"/>',
};

/** An ICO file holding one PNG per size (Windows Vista and later read PNG entries). */
function ico(pngs) {
  const header = Buffer.alloc(6 + 16 * pngs.length);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(pngs.length, 4);
  let offset = header.length;
  pngs.forEach(({ size, png }, i) => {
    const at = 6 + 16 * i;
    header.writeUInt8(size >= 256 ? 0 : size, at);
    header.writeUInt8(size >= 256 ? 0 : size, at + 1);
    header.writeUInt8(0, at + 2);
    header.writeUInt8(0, at + 3);
    header.writeUInt16LE(1, at + 4);
    header.writeUInt16LE(32, at + 6);
    header.writeUInt32LE(png.length, at + 8);
    header.writeUInt32LE(offset, at + 12);
    offset += png.length;
  });
  return Buffer.concat([header, ...pngs.map(({ png }) => png)]);
}

function shellIcons() {
  const icons = join(root, 'src-tauri/icons');
  // 22 pt menu bar icon at 2x.
  writeFileSync(join(icons, 'tray-template.png'), renderSvg(TRAY_TEMPLATE, 44));
  for (const [name, paths] of Object.entries(THUMBS)) {
    const svg = glyph(paths);
    writeFileSync(
      join(icons, `thumb-${name}.ico`),
      ico([16, 32].map((size) => ({ size, png: renderSvg(svg, size) }))),
    );
  }
}

// Runs the local Tauri CLI through node directly: no shell, so no quoting or npx.cmd lookup on Windows.
const cli = join(root, 'node_modules/@tauri-apps/cli/tauri.js');
const tauri = (args) => execFileSync(process.execPath, [cli, ...args], { cwd: root, stdio: 'inherit' });

if (!process.argv.includes('--shell')) {
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
}
shellIcons();
