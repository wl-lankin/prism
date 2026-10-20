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
  'view.album.lead': 'Album details appear here.',
  'view.artist.lead': 'Artist details appear here.',
  'view.favorites.lead': 'Favorites from every source arrive in a later version.',
  'view.playlists.lead': 'Playlists arrive in a later version.',
  'view.count.albums': '{count} albums',
  'view.count.albums.one': '1 album',
  'view.count.artists': '{count} artists',
  'view.count.artists.one': '1 artist',
  'view.count.songs': '{count} songs',
  'view.count.songs.one': '1 song',
  'view.count.tracks': '{count} tracks',
  'view.count.tracks.one': '1 track',

  'time.minutes': '{m} min',
  'time.hours': '{h} h {m} min',

  'library.emptyTitle': 'Your library is empty',
  'library.emptyLead': 'Add a folder with music. Prism reads the tags and covers and keeps the folder in sync when files change.',
  'library.addFolder': 'Add music folder',
  'library.chooseFolder': 'Choose a music folder',
  'library.looking': 'Looking for music files',
  'library.scanning': 'Reading {done} of {total} files',
  'library.addFailed': 'The folder could not be added: {message}',
  'library.loadFailed': 'The library could not be loaded: {message}',

  'album.play': 'Play',
  'album.shuffle': 'Shuffle',
  'album.playTitle': 'Play {title}',
  'album.disc': 'Disc {n}',
  'album.missing': 'This album is no longer in your library.',
  'album.back': 'Back',
  'album.nowPlaying': 'Now playing',

  'songs.title': 'Title',
  'songs.artist': 'Artist',
  'songs.album': 'Album',
  'songs.time': 'Time',

  'artist.missing': 'This artist is no longer in your library.',

  'search.open': 'Search',
  'search.label': 'Search the library',
  'search.placeholder': 'Albums, artists and songs',
  'search.hint': 'Type to search your library.',
  'search.empty': 'Nothing matches "{query}".',
  'search.failed': 'Search failed: {message}',
  'search.keys': 'Arrows to move, Enter to open, Esc to close',

  'drop.hint': 'Drop to play',
  'drop.lead': 'Files play right away without being added to your library.',
  'drop.none': 'None of the dropped files can be played.',
  'drop.failed': 'The dropped files could not be played: {message}',
  'startup.quarantined': 'Your library file was damaged. It was set aside as {file} and Prism started with an empty library. Add your folders again to rebuild it.',

  'settings.library': 'Library',
  'settings.libraryLead': 'Folders Prism reads music from. When files change, the library follows on its own.',
  'settings.folders.empty': 'No folders yet. Add a folder with music to fill your library.',
  'settings.folders.online': 'Online',
  'settings.folders.offline': 'Offline',
  'settings.folders.offlineHint': 'Not reachable right now. Its tracks stay in your library until it is back.',
  'settings.folders.remove': 'Remove',
  'settings.folders.removeLabel': 'Remove {path}',
  'settings.folders.confirmRemove': 'Remove from library',
  'settings.folders.cancel': 'Keep',
  'settings.folders.rescan': 'Rescan all',
  'settings.folders.removeFailed': 'The folder could not be removed: {message}',
  'settings.folders.rescanFailed': 'Rescanning failed: {message}',
  'settings.folders.loadFailed': 'The folders could not be loaded: {message}',
  'settings.playback': 'Playback',
  'settings.output': 'Output device',
  'settings.outputHint': 'Switches right away and keeps your place in the track.',
  'settings.output.default': 'System default',
  'settings.output.missing': 'not connected',
  'settings.gapless': 'Gapless playback',
  'settings.gaplessHint': 'Opens the next track early, so live albums and mixes play without a pause.',
  'settings.replayGain': 'ReplayGain',
  'settings.replayGainHint': 'Evens out loudness with the volume tags in your files. Album keeps the steps between the songs of one album.',
  'settings.replayGain.off': 'Off',
  'settings.replayGain.track': 'Track',
  'settings.replayGain.album': 'Album',
  'settings.language': 'Language',
  'settings.languageLabel': 'Display language',
  'settings.languageHint': 'System follows the language of your computer.',
  'settings.language.system': 'System ({name})',
  'settings.language.en': 'English',
  'settings.language.de': 'Deutsch',
  'settings.saveFailed': 'The setting could not be saved: {message}',

  'about.version': 'Version {version}',
  'about.tagline': 'One player, every color of sound.',
  'about.madeBy': 'Made by',
  'about.github': 'Source code on GitHub',
  'about.licenses': 'Licenses',
  'about.licensesLead': 'Prism is MIT licensed and built on these open-source components.',
  'about.external': 'used as an external program',
  'about.close': 'Close',
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
  'view.album.lead': 'Hier erscheinen die Details zum Album.',
  'view.artist.lead': 'Hier erscheinen die Details zum Interpreten.',
  'view.favorites.lead': 'Favoriten aus allen Quellen kommen in einer späteren Version.',
  'view.playlists.lead': 'Playlists kommen in einer späteren Version.',
  'view.count.albums': '{count} Alben',
  'view.count.albums.one': '1 Album',
  'view.count.artists': '{count} Interpreten',
  'view.count.artists.one': '1 Interpret',
  'view.count.songs': '{count} Titel',
  'view.count.songs.one': '1 Titel',
  'view.count.tracks': '{count} Titel',
  'view.count.tracks.one': '1 Titel',

  'time.minutes': '{m} Min.',
  'time.hours': '{h} Std. {m} Min.',

  'library.emptyTitle': 'Deine Mediathek ist leer',
  'library.emptyLead': 'Füge einen Ordner mit Musik hinzu. Prism liest Tags und Cover und hält den Ordner aktuell, wenn sich Dateien ändern.',
  'library.addFolder': 'Musikordner hinzufügen',
  'library.chooseFolder': 'Musikordner auswählen',
  'library.looking': 'Suche nach Musikdateien',
  'library.scanning': 'Lese {done} von {total} Dateien',
  'library.addFailed': 'Der Ordner konnte nicht hinzugefügt werden: {message}',
  'library.loadFailed': 'Die Mediathek konnte nicht geladen werden: {message}',

  'album.play': 'Abspielen',
  'album.shuffle': 'Zufällig',
  'album.playTitle': '{title} abspielen',
  'album.disc': 'CD {n}',
  'album.missing': 'Dieses Album ist nicht mehr in deiner Mediathek.',
  'album.back': 'Zurück',
  'album.nowPlaying': 'Läuft gerade',

  'songs.title': 'Titel',
  'songs.artist': 'Interpret',
  'songs.album': 'Album',
  'songs.time': 'Dauer',

  'artist.missing': 'Dieser Interpret ist nicht mehr in deiner Mediathek.',

  'search.open': 'Suchen',
  'search.label': 'Mediathek durchsuchen',
  'search.placeholder': 'Alben, Interpreten und Titel',
  'search.hint': 'Tippe, um deine Mediathek zu durchsuchen.',
  'search.empty': 'Keine Treffer für „{query}“.',
  'search.failed': 'Suche fehlgeschlagen: {message}',
  'search.keys': 'Pfeiltasten zum Wählen, Enter zum Öffnen, Esc zum Schließen',

  'drop.hint': 'Zum Abspielen loslassen',
  'drop.lead': 'Die Dateien laufen sofort, ohne in die Mediathek aufgenommen zu werden.',
  'drop.none': 'Keine der Dateien kann abgespielt werden.',
  'drop.failed': 'Die Dateien konnten nicht abgespielt werden: {message}',
  'startup.quarantined': 'Deine Bibliotheksdatei war beschädigt. Sie wurde als {file} beiseitegelegt und Prism startet mit einer leeren Mediathek. Füge deine Ordner erneut hinzu, um sie neu aufzubauen.',

  'settings.library': 'Mediathek',
  'settings.libraryLead': 'Ordner, aus denen Prism Musik liest. Ändern sich Dateien, zieht die Mediathek von selbst nach.',
  'settings.folders.empty': 'Noch keine Ordner. Füge einen Ordner mit Musik hinzu, um deine Mediathek zu füllen.',
  'settings.folders.online': 'Verfügbar',
  'settings.folders.offline': 'Nicht erreichbar',
  'settings.folders.offlineHint': 'Gerade nicht erreichbar. Die Titel bleiben in deiner Mediathek, bis der Ordner zurück ist.',
  'settings.folders.remove': 'Entfernen',
  'settings.folders.removeLabel': '{path} entfernen',
  'settings.folders.confirmRemove': 'Aus Mediathek entfernen',
  'settings.folders.cancel': 'Behalten',
  'settings.folders.rescan': 'Alle neu einlesen',
  'settings.folders.removeFailed': 'Der Ordner konnte nicht entfernt werden: {message}',
  'settings.folders.rescanFailed': 'Das Einlesen ist fehlgeschlagen: {message}',
  'settings.folders.loadFailed': 'Die Ordner konnten nicht geladen werden: {message}',
  'settings.playback': 'Wiedergabe',
  'settings.output': 'Ausgabegerät',
  'settings.outputHint': 'Wechselt sofort und behält die Stelle im Titel bei.',
  'settings.output.default': 'Systemstandard',
  'settings.output.missing': 'nicht verbunden',
  'settings.gapless': 'Lückenlose Wiedergabe',
  'settings.gaplessHint': 'Öffnet den nächsten Titel früher, damit Live-Alben und Mixe ohne Pause laufen.',
  'settings.replayGain': 'ReplayGain',
  'settings.replayGainHint': 'Gleicht die Lautstärke über die Lautstärke-Tags deiner Dateien an. Album behält die Abstufungen zwischen den Titeln eines Albums.',
  'settings.replayGain.off': 'Aus',
  'settings.replayGain.track': 'Titel',
  'settings.replayGain.album': 'Album',
  'settings.language': 'Sprache',
  'settings.languageLabel': 'Anzeigesprache',
  'settings.languageHint': 'System folgt der Sprache deines Computers.',
  'settings.language.system': 'System ({name})',
  'settings.language.en': 'English',
  'settings.language.de': 'Deutsch',
  'settings.saveFailed': 'Die Einstellung konnte nicht gespeichert werden: {message}',

  'about.version': 'Version {version}',
  'about.tagline': 'Ein Player, jede Farbe des Klangs.',
  'about.madeBy': 'Entwickelt von',
  'about.github': 'Quellcode auf GitHub',
  'about.licenses': 'Lizenzen',
  'about.licensesLead': 'Prism steht unter der MIT-Lizenz und baut auf diesen Open-Source-Komponenten auf.',
  'about.external': 'als externes Programm genutzt',
  'about.close': 'Schließen',
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
if (typeof document !== 'undefined') document.documentElement.lang = current;

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

/** `key` with `{count}` filled in (grouped digits), using `<key>.one` for exactly one. */
export function tc(key: Key | (string & {}), count: number): string {
  subscribe();
  const one = `${key}.one`;
  if (count === 1 && one in strings.en) return t(one);
  return t(key, { count: count.toLocaleString(current) });
}

/** The string for `key` in the current language, with `{name}` placeholders filled from `vars`. */
export function t(key: Key | (string & {}), vars?: Record<string, string | number>): string {
  subscribe();
  const template = strings[current][key] ?? strings.en[key] ?? key;
  if (!vars) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) => (name in vars ? String(vars[name]) : match));
}
