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
  'ring.flowHint': 'Flow from your favorites. Right-click or hold for more.',
  'ring.play': 'Play',
  'ring.pause': 'Pause',

  'transport.idleTitle': 'Nothing playing',
  'transport.idleHint': 'Click Flow for an endless mix of your favorites',
  'transport.loading': 'Loading',
  'transport.previous': 'Previous',
  'transport.next': 'Next',
  'transport.seek': 'Position',
  'transport.volume': 'Volume',
  'transport.mute': 'Mute',
  'transport.unmute': 'Unmute',

  'tray.open': 'Open Prism',
  'tray.playPause': 'Play/Pause',
  'tray.next': 'Next',
  'tray.previous': 'Previous',
  'tray.quit': 'Quit Prism',
  'mini.label': 'Mini player',
  'mini.open': 'Open Prism',
  'mini.openNowPlaying': 'Show what is playing in Prism',
  'mini.idleTitle': 'Ready when you are',
  'mini.idleHint': 'Flow mixes your favorites from every source',
  'mini.emptyLibrary': 'Your library is empty. Open Prism to add a music folder.',

  'flow.failed': 'Playback could not start: {message}',
  'flow.noFavorites': 'No favorites yet, so Flow plays your whole library.',
  'flow.menu': 'Start a Flow',
  'flow.fromFavorites': 'Flow from favorites',
  'flow.fromLibrary': 'Flow from the whole library',
  'flow.fromGenre': 'Flow from a genre',
  'flow.genreFilter': 'Filter genres',
  'flow.noGenres': 'No genres in your library',
  'flow.noGenreMatch': 'No genre matches',
  'flow.genresFailed': 'Genres could not be loaded: {message}',
  'flow.back': 'Back',

  'view.home.lead': 'Albums from your library appear here.',
  'view.albums.lead': 'Albums from your library appear here.',
  'view.artists.lead': 'Everyone in your library, sorted by name.',
  'view.songs.lead': 'Every track in your library, in one list.',
  'view.album.lead': 'Album details appear here.',
  'view.artist.lead': 'Artist details appear here.',
  'view.playlists.lead': 'Your playlists appear here.',
  'view.count.albums': '{count} albums',
  'view.count.albums.one': '1 album',
  'view.count.artists': '{count} artists',
  'view.count.artists.one': '1 artist',
  'view.count.songs': '{count} songs',
  'view.count.songs.one': '1 song',
  'view.count.tracks': '{count} tracks',
  'view.count.tracks.one': '1 track',
  'view.count.playlists': '{count} playlists',
  'view.count.playlists.one': '1 playlist',

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
  'row.play': 'Play {name}',
  'row.pause': 'Pause {name}',
  'row.resume': 'Resume {name}',

  'nowPlaying.title': 'Now playing',
  'nowPlaying.open': 'Show what is playing and the queue',
  'nowPlaying.close': 'Back',
  'nowPlaying.queue': 'Queue',
  'nowPlaying.position': '{n} of {total}',
  'nowPlaying.flow': 'Flow · {variant}',
  'nowPlaying.flowFavorites': 'Favorites',
  'nowPlaying.flowLibrary': 'Library',
  'nowPlaying.missing': 'No longer in your library',
  'nowPlaying.emptyLead': 'Play an album, a song or Flow, and it shows up here with everything that comes next.',

  'songs.title': 'Title',
  'songs.artist': 'Artist',
  'songs.album': 'Album',
  'songs.time': 'Time',

  'artist.missing': 'This artist is no longer in your library.',

  'favorites.toggle': 'Favorite',
  'favorites.toggleItem': 'Favorite: {name}',
  'favorites.add': 'Add to favorites',
  'favorites.remove': 'Remove from favorites',
  'favorites.failed': 'The favorite could not be saved: {message}',
  'favorites.loadFailed': 'The favorites could not be loaded: {message}',
  'favorites.kinds': 'Favorites by kind',
  'favorites.all': 'All',
  'favorites.empty.track': 'No favorite songs yet',
  'favorites.empty.trackLead': 'Click the heart next to a song, or in the player bar while it plays, to keep it here.',
  'favorites.empty.album': 'No favorite albums yet',
  'favorites.empty.albumLead': 'Open an album and click the heart next to Play.',
  'favorites.empty.artist': 'No favorite artists yet',
  'favorites.empty.artistLead': 'Open an artist and click the heart next to Play.',
  'favorites.emptySource': 'None of these favorites come from {source}.',
  'favorites.browse': 'Browse albums',

  'playlists.new': 'New playlist',
  'playlists.newMenu': 'New playlist...',
  'playlists.defaultName': 'New playlist',
  'playlists.defaultNameN': 'New playlist {n}',
  'playlists.nameLabel': 'Playlist name',
  'playlists.empty': 'No playlists yet',
  'playlists.emptyLead': 'Gather songs from any album in the order you like. Right-click a song or an album and choose Add to playlist.',
  'playlists.loadFailed': 'The playlists could not be loaded: {message}',
  'playlists.createFailed': 'The playlist could not be created: {message}',
  'playlists.added': 'Added to {name}',
  'playlists.addFailed': 'The songs could not be added: {message}',
  'playlist.rename': 'Rename playlist',
  'playlist.renameFailed': 'The playlist could not be renamed: {message}',
  'playlist.changeFailed': 'The playlist could not be changed: {message}',
  'playlist.delete': 'Delete playlist',
  'playlist.deleteConfirm': 'Delete',
  'playlist.keep': 'Keep',
  'playlist.deleteFailed': 'The playlist could not be deleted: {message}',
  'playlist.empty': 'This playlist is empty',
  'playlist.emptyLead': 'Right-click songs or albums anywhere in your library and choose Add to playlist.',
  'playlist.missing': 'Not in your library',
  'playlist.unavailable': '{count} unavailable',
  'playlist.gone': 'This playlist no longer exists.',
  'playlist.nothingPlayable': 'None of the songs in this playlist are in your library.',
  'playlist.reorder': 'Drag to reorder',

  'menu.label': 'Song actions',
  'menu.addToPlaylist': 'Add to playlist',
  'menu.addAlbumToPlaylist': 'Add album to playlist',
  'menu.goToAlbum': 'Go to album',
  'menu.goToArtist': 'Go to artist',
  'menu.removeFromPlaylist': 'Remove from playlist',

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
  'settings.crossfade': 'Crossfade',
  'settings.crossfadeHint': 'Blends the end of one track into the next. Inside Flow it only blends where the album does not continue on its own.',
  'settings.crossfade.off': 'Off',
  'settings.crossfade.flow': 'In Flow',
  'settings.crossfade.always': 'Always',
  'settings.crossfadeLength': 'Crossfade length',
  'settings.crossfadeLengthHint': 'How long two tracks overlap.',
  'settings.seconds': '{count} s',
  'settings.mini': 'Mini player',
  'settings.miniOnTop': 'Keep on top',
  'settings.miniOnTopHint': 'The mini player stays above other windows.',
  'settings.shortcut': 'Shortcut',
  'settings.shortcutHint': 'Shows or hides the mini player from any app. Backspace goes back to the default.',
  'settings.shortcut.record': 'Press the keys',
  'settings.shortcut.label': 'Mini player shortcut: {keys}. Press to change.',
  'settings.shortcut.invalid': 'That key combination cannot be used as a shortcut.',
  'settings.shortcut.taken': '{keys} is already used by another app. Pick another combination.',
  'settings.covers': 'Covers',
  'settings.coverLookup': 'Look up missing covers online',
  'settings.coverLookupHint': 'Asks MusicBrainz and the Cover Art Archive for albums without a cover. Only artist and album names are sent.',
  'settings.system': 'System',
  'settings.startWithSystem': 'Start with system',
  'settings.startWithSystemHint': 'Prism starts quietly when you sign in: only its tray icon appears, no window opens.',
  'settings.startWithSystemHintMac': 'Prism starts quietly when you log in: only its Dock and menu bar icons appear, no window opens.',
  'settings.startWithSystemFailed': 'The start with the system could not be changed: {message}',

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
  'ring.flowHint': 'Flow aus deinen Favoriten. Rechtsklick oder gedrückt halten für mehr.',
  'ring.play': 'Abspielen',
  'ring.pause': 'Pause',

  'transport.idleTitle': 'Keine Wiedergabe',
  'transport.idleHint': 'Klicke auf Flow für einen endlosen Mix deiner Favoriten',
  'transport.loading': 'Wird geladen',
  'transport.previous': 'Zurück',
  'transport.next': 'Weiter',
  'transport.seek': 'Position',
  'transport.volume': 'Lautstärke',
  'transport.mute': 'Stumm',
  'transport.unmute': 'Ton an',

  'tray.open': 'Prism öffnen',
  'tray.playPause': 'Wiedergabe/Pause',
  'tray.next': 'Weiter',
  'tray.previous': 'Zurück',
  'tray.quit': 'Prism beenden',
  'mini.label': 'Mini-Player',
  'mini.open': 'Prism öffnen',
  'mini.openNowPlaying': 'Laufenden Titel in Prism zeigen',
  'mini.idleTitle': 'Bereit, wenn du es bist',
  'mini.idleHint': 'Flow mischt deine Favoriten aus allen Quellen',
  'mini.emptyLibrary': 'Deine Mediathek ist leer. Öffne Prism, um einen Musikordner hinzuzufügen.',

  'flow.failed': 'Wiedergabe konnte nicht starten: {message}',
  'flow.noFavorites': 'Noch keine Favoriten, daher spielt Flow deine ganze Mediathek.',
  'flow.menu': 'Flow starten',
  'flow.fromFavorites': 'Flow aus Favoriten',
  'flow.fromLibrary': 'Flow aus der ganzen Mediathek',
  'flow.fromGenre': 'Flow aus einem Genre',
  'flow.genreFilter': 'Genres filtern',
  'flow.noGenres': 'Keine Genres in deiner Mediathek',
  'flow.noGenreMatch': 'Kein Genre passt',
  'flow.genresFailed': 'Genres konnten nicht geladen werden: {message}',
  'flow.back': 'Zurück',

  'view.home.lead': 'Hier erscheinen die Alben aus deiner Mediathek.',
  'view.albums.lead': 'Hier erscheinen die Alben aus deiner Mediathek.',
  'view.artists.lead': 'Alle Interpreten deiner Mediathek, nach Namen sortiert.',
  'view.songs.lead': 'Alle Titel deiner Mediathek in einer Liste.',
  'view.album.lead': 'Hier erscheinen die Details zum Album.',
  'view.artist.lead': 'Hier erscheinen die Details zum Interpreten.',
  'view.playlists.lead': 'Hier erscheinen deine Playlists.',
  'view.count.albums': '{count} Alben',
  'view.count.albums.one': '1 Album',
  'view.count.artists': '{count} Interpreten',
  'view.count.artists.one': '1 Interpret',
  'view.count.songs': '{count} Titel',
  'view.count.songs.one': '1 Titel',
  'view.count.tracks': '{count} Titel',
  'view.count.tracks.one': '1 Titel',
  'view.count.playlists': '{count} Playlists',
  'view.count.playlists.one': '1 Playlist',

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
  'row.play': '{name} abspielen',
  'row.pause': '{name} pausieren',
  'row.resume': '{name} fortsetzen',

  'nowPlaying.title': 'Läuft gerade',
  'nowPlaying.open': 'Wiedergabe und Warteschlange zeigen',
  'nowPlaying.close': 'Zurück',
  'nowPlaying.queue': 'Warteschlange',
  'nowPlaying.position': '{n} von {total}',
  'nowPlaying.flow': 'Flow · {variant}',
  'nowPlaying.flowFavorites': 'Favoriten',
  'nowPlaying.flowLibrary': 'Mediathek',
  'nowPlaying.missing': 'Nicht mehr in deiner Mediathek',
  'nowPlaying.emptyLead': 'Spiel ein Album, einen Song oder Flow ab, dann steht es hier, mit allem, was danach kommt.',

  'songs.title': 'Titel',
  'songs.artist': 'Interpret',
  'songs.album': 'Album',
  'songs.time': 'Dauer',

  'artist.missing': 'Dieser Interpret ist nicht mehr in deiner Mediathek.',

  'favorites.toggle': 'Favorit',
  'favorites.toggleItem': 'Favorit: {name}',
  'favorites.add': 'Zu Favoriten hinzufügen',
  'favorites.remove': 'Aus Favoriten entfernen',
  'favorites.failed': 'Der Favorit konnte nicht gespeichert werden: {message}',
  'favorites.loadFailed': 'Die Favoriten konnten nicht geladen werden: {message}',
  'favorites.kinds': 'Favoriten nach Art',
  'favorites.all': 'Alle',
  'favorites.empty.track': 'Noch keine Lieblingstitel',
  'favorites.empty.trackLead': 'Klicke auf das Herz neben einem Titel oder in der Wiedergabeleiste, um ihn hier zu sammeln.',
  'favorites.empty.album': 'Noch keine Lieblingsalben',
  'favorites.empty.albumLead': 'Öffne ein Album und klicke auf das Herz neben Abspielen.',
  'favorites.empty.artist': 'Noch keine Lieblingsinterpreten',
  'favorites.empty.artistLead': 'Öffne einen Interpreten und klicke auf das Herz neben Abspielen.',
  'favorites.emptySource': 'Keiner dieser Favoriten stammt aus {source}.',
  'favorites.browse': 'Alben ansehen',

  'playlists.new': 'Neue Playlist',
  'playlists.newMenu': 'Neue Playlist...',
  'playlists.defaultName': 'Neue Playlist',
  'playlists.defaultNameN': 'Neue Playlist {n}',
  'playlists.nameLabel': 'Name der Playlist',
  'playlists.empty': 'Noch keine Playlists',
  'playlists.emptyLead': 'Sammle Titel aus beliebigen Alben in deiner eigenen Reihenfolge. Klicke mit der rechten Maustaste auf einen Titel oder ein Album und wähle Zur Playlist hinzufügen.',
  'playlists.loadFailed': 'Die Playlists konnten nicht geladen werden: {message}',
  'playlists.createFailed': 'Die Playlist konnte nicht erstellt werden: {message}',
  'playlists.added': 'Zu {name} hinzugefügt',
  'playlists.addFailed': 'Die Titel konnten nicht hinzugefügt werden: {message}',
  'playlist.rename': 'Playlist umbenennen',
  'playlist.renameFailed': 'Die Playlist konnte nicht umbenannt werden: {message}',
  'playlist.changeFailed': 'Die Playlist konnte nicht geändert werden: {message}',
  'playlist.delete': 'Playlist löschen',
  'playlist.deleteConfirm': 'Löschen',
  'playlist.keep': 'Behalten',
  'playlist.deleteFailed': 'Die Playlist konnte nicht gelöscht werden: {message}',
  'playlist.empty': 'Diese Playlist ist leer',
  'playlist.emptyLead': 'Klicke irgendwo in deiner Mediathek mit der rechten Maustaste auf Titel oder Alben und wähle Zur Playlist hinzufügen.',
  'playlist.missing': 'Nicht in deiner Mediathek',
  'playlist.unavailable': '{count} nicht verfügbar',
  'playlist.gone': 'Diese Playlist gibt es nicht mehr.',
  'playlist.nothingPlayable': 'Keiner der Titel dieser Playlist ist in deiner Mediathek.',
  'playlist.reorder': 'Zum Umsortieren ziehen',

  'menu.label': 'Titelaktionen',
  'menu.addToPlaylist': 'Zur Playlist hinzufügen',
  'menu.addAlbumToPlaylist': 'Album zur Playlist hinzufügen',
  'menu.goToAlbum': 'Zum Album',
  'menu.goToArtist': 'Zum Interpreten',
  'menu.removeFromPlaylist': 'Aus Playlist entfernen',

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
  'settings.crossfade': 'Überblenden',
  'settings.crossfadeHint': 'Blendet das Ende eines Titels in den nächsten über. Im Flow wird nur dort überblendet, wo das Album nicht von selbst weiterläuft.',
  'settings.crossfade.off': 'Aus',
  'settings.crossfade.flow': 'Im Flow',
  'settings.crossfade.always': 'Immer',
  'settings.crossfadeLength': 'Dauer der Überblendung',
  'settings.crossfadeLengthHint': 'Wie lange sich zwei Titel überlagern.',
  'settings.seconds': '{count} s',
  'settings.mini': 'Mini-Player',
  'settings.miniOnTop': 'Immer im Vordergrund',
  'settings.miniOnTopHint': 'Der Mini-Player bleibt über anderen Fenstern.',
  'settings.shortcut': 'Tastenkürzel',
  'settings.shortcutHint': 'Zeigt oder versteckt den Mini-Player aus jeder App heraus. Rücktaste stellt den Standard wieder her.',
  'settings.shortcut.record': 'Tasten drücken',
  'settings.shortcut.label': 'Tastenkürzel für den Mini-Player: {keys}. Zum Ändern drücken.',
  'settings.shortcut.invalid': 'Diese Tastenkombination kann nicht als Tastenkürzel dienen.',
  'settings.shortcut.taken': '{keys} wird schon von einer anderen App verwendet. Wähle eine andere Kombination.',
  'settings.covers': 'Cover',
  'settings.coverLookup': 'Fehlende Cover online suchen',
  'settings.coverLookupHint': 'Fragt MusicBrainz und das Cover Art Archive nach Alben ohne Cover. Gesendet werden nur Künstler- und Albumnamen.',
  'settings.system': 'System',
  'settings.startWithSystem': 'Mit dem System starten',
  'settings.startWithSystemHint': 'Prism startet still bei der Anmeldung: Nur das Symbol im Infobereich erscheint, kein Fenster öffnet sich.',
  'settings.startWithSystemHintMac': 'Prism startet still bei der Anmeldung: Nur die Symbole im Dock und in der Menüleiste erscheinen, kein Fenster öffnet sich.',
  'settings.startWithSystemFailed': 'Der Start mit dem System ließ sich nicht ändern: {message}',

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
