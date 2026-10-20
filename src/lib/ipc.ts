export type CoverSize = 96 | 300 | 600;

/** Windows WebView2 serves custom schemes as `http://<scheme>.localhost`; others keep `<scheme>://localhost`. */
function isWindows(): boolean {
  return /Win/i.test(navigator.userAgent);
}

export function coverUrl(id: string | null, size: CoverSize): string | null {
  if (!id) return null;
  const base = isWindows() ? 'http://prism.localhost' : 'prism://localhost';
  return `${base}/cover/${id}/${size}`;
}
