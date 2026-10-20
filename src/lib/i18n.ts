import { createSubscriber } from 'svelte/reactivity';

export type Lang = 'en' | 'de';
export type LangPref = Lang | 'system';

const en = {
  'app.name': 'Prism',
  'app.about': 'About Prism',

  'nav.home': 'Home',
  'nav.favorites': 'Favorites',
  'nav.playlists': 'Playlists',
  'nav.albums': 'Albums',
  'nav.artists': 'Artists',
  'nav.songs': 'Songs',
  'nav.settings': 'Settings',
  'nav.library': 'Library',
  'nav.sources': 'Sources',
  'nav.comingSoon': 'Coming soon',

  'source.local': 'Local files',
  'source.navidrome': 'Navidrome',
  'source.youtube': 'YouTube Music',
  'source.radio': 'Radio',

  'window.minimize': 'Minimize',
  'window.maximize': 'Maximize',
  'window.restore': 'Restore',
  'window.close': 'Close',

  'ring.flow': 'Flow',
  'ring.flowHint': 'Play all albums shuffled',
  'ring.play': 'Play',
  'ring.pause': 'Pause',

  'transport.idleTitle': 'Nothing playing',
  'transport.idleHint': 'Click Flow to shuffle your library',
  'transport.loading': 'Loading',
  'transport.previous': 'Previous',
  'transport.next': 'Next',
  'transport.seek': 'Position',
  'transport.volume': 'Volume',
  'transport.mute': 'Mute',
  'transport.unmute': 'Unmute',
  'transport.favorite': 'Add to favorites',

  'flow.failed': 'Playback could not start: {message}',

  'view.home.lead': 'Albums from your library appear here.',
  'view.albums.lead': 'Albums from your library appear here.',
  'view.artists.lead': 'Everyone in your library, sorted by name.',
  'view.songs.lead': 'Every track in your library, in one list.',
  'view.settings.lead': 'Library folders, playback and language settings appear here.',
  'view.album.lead': 'Album details appear here.',
  'view.artist.lead': 'Artist details appear here.',
  'view.favorites.lead': 'Favorites from every source arrive in a later version.',
  'view.playlists.lead': 'Playlists arrive in a later version.',
  'view.count.albums': '{count} albums',
} as const;

export type Key = keyof typeof en;

const de: Record<Key, string> = {
  'app.name': 'Prism',
  'app.about': 'Über Prism',

  'nav.home': 'Start',
  'nav.favorites': 'Favoriten',
  'nav.playlists': 'Playlists',
  'nav.albums': 'Alben',
  'nav.artists': 'Interpreten',
  'nav.songs': 'Titel',
  'nav.settings': 'Einstellungen',
  'nav.library': 'Mediathek',
  'nav.sources': 'Quellen',
  'nav.comingSoon': 'Kommt bald',

  'source.local': 'Lokale Dateien',
  'source.navidrome': 'Navidrome',
  'source.youtube': 'YouTube Music',
  'source.radio': 'Radio',

  'window.minimize': 'Minimieren',
  'window.maximize': 'Maximieren',
  'window.restore': 'Wiederherstellen',
  'window.close': 'Schließen',

  'ring.flow': 'Flow',
  'ring.flowHint': 'Alle Alben gemischt abspielen',
  'ring.play': 'Abspielen',
  'ring.pause': 'Pause',

  'transport.idleTitle': 'Keine Wiedergabe',
  'transport.idleHint': 'Klicke auf Flow, um deine Mediathek zu mischen',
  'transport.loading': 'Wird geladen',
  'transport.previous': 'Zurück',
  'transport.next': 'Weiter',
  'transport.seek': 'Position',
  'transport.volume': 'Lautstärke',
  'transport.mute': 'Stumm',
  'transport.unmute': 'Ton an',
  'transport.favorite': 'Zu Favoriten hinzufügen',

  'flow.failed': 'Wiedergabe konnte nicht starten: {message}',

  'view.home.lead': 'Hier erscheinen die Alben aus deiner Mediathek.',
  'view.albums.lead': 'Hier erscheinen die Alben aus deiner Mediathek.',
  'view.artists.lead': 'Alle Interpreten deiner Mediathek, nach Namen sortiert.',
  'view.songs.lead': 'Alle Titel deiner Mediathek in einer Liste.',
  'view.settings.lead': 'Hier erscheinen Musikordner, Wiedergabe und Sprache.',
  'view.album.lead': 'Hier erscheinen die Details zum Album.',
  'view.artist.lead': 'Hier erscheinen die Details zum Interpreten.',
  'view.favorites.lead': 'Favoriten aus allen Quellen kommen in einer späteren Version.',
  'view.playlists.lead': 'Playlists kommen in einer späteren Version.',
  'view.count.albums': '{count} Alben',
};

/** The string tables, exported for tests. */
export const strings: Record<Lang, Record<string, string>> = { en, de };

/** The first supported language in a preference list (`de*` is German); English otherwise. */
export function systemLang(languages: readonly string[]): Lang {
  for (const tag of languages) {
    const base = tag.toLowerCase().split('-')[0];
    if (base === 'de' || base === 'en') return base;
  }
  return 'en';
}

function detect(): Lang {
  if (typeof navigator === 'undefined') return 'en';
  return systemLang(navigator.languages?.length ? navigator.languages : [navigator.language ?? 'en']);
}

let pref: LangPref = 'system';
let current: Lang = detect();

// Reading `t()` inside a Svelte effect or template subscribes it to language changes.
const listeners = new Set<() => void>();
const subscribe = createSubscriber((update) => {
  listeners.add(update);
  return () => listeners.delete(update);
});

/** The language in use. */
export function lang(): Lang {
  subscribe();
  return current;
}

/** The stored preference: a language or 'system'. */
export function langPref(): LangPref {
  subscribe();
  return pref;
}

/** Switches the UI language; 'system' follows `navigator.languages`. */
export function setLang(next: LangPref): void {
  pref = next;
  current = next === 'system' ? detect() : next;
  if (typeof document !== 'undefined') document.documentElement.lang = current;
  for (const update of listeners) update();
}

/** The string for `key` in the current language, with `{name}` placeholders filled from `vars`. */
export function t(key: Key | (string & {}), vars?: Record<string, string | number>): string {
  subscribe();
  const template = strings[current][key] ?? strings.en[key] ?? key;
  if (!vars) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) => (name in vars ? String(vars[name]) : match));
}
