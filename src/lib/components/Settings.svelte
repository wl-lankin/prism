<script lang="ts">
  import { tick } from 'svelte';
  import { getVersion } from '@tauri-apps/api/app';
  import { open } from '@tauri-apps/plugin-dialog';
  import logoUrl from '../../../assets/logo.svg';
  import { lang, systemLang, t, tc } from '../i18n';
  import {
    audioDevices,
    libraryAddFolder,
    libraryFolders,
    libraryRemoveFolder,
    libraryRescan,
    navidromeConnect,
    navidromeDisconnect,
    navidromeSync,
    navidromeTest,
  } from '../ipc';
  import { library, reloadLibrary } from '../stores/library.svelte';
  import { nav, overlays } from '../stores/nav.svelte';
  import { navidrome, navidromeErrorText } from '../stores/navidrome.svelte';
  import { listCountries, listRegions, loadNear, radio, redetectPlace } from '../stores/radio.svelte';
  import { settings, updateSettings } from '../stores/settings.svelte';
  import type {
    CrossfadeMode,
    Folder,
    LanguagePref,
    RadioCountry,
    RadioLocation,
    RadioRegion,
    ReplayGainMode,
    ServerInfo,
    Settings,
    StreamQuality,
  } from '../types';
  import Icon from './Icon.svelte';
  import Select, { type SelectOption } from './Select.svelte';
  import ShortcutInput, { IS_MAC, describeShortcut } from './ShortcutInput.svelte';
  import Segmented from './Segmented.svelte';
  import Slider from './Slider.svelte';
  import ViewHead from './ViewHead.svelte';

  /** Settings: library folders, playback, the mini player, covers, the system start and
   *  language. Every change is saved at once. */

  const DEFAULT_DEVICE = '';

  let folders = $state.raw<Folder[] | null>(null);
  let folderError = $state<string | null>(null);
  let adding = $state(false);
  let rescanning = $state(false);
  /** The folder whose removal waits for a second click. */
  let confirming = $state<string | null>(null);
  let devices = $state.raw<string[]>([]);
  let saveError = $state<string | null>(null);
  let version = $state('');

  const scan = $derived(library.scan);
  const busy = $derived(adding || rescanning || !!scan);
  const share = $derived(scan && scan.total > 0 ? Math.min(1, scan.done / scan.total) : 0);

  async function loadFolders() {
    try {
      folders = (await libraryFolders()) ?? [];
      folderError = null;
    } catch (err) {
      folderError = t('settings.folders.loadFailed', { message: String(err) });
    }
  }

  // `go('settings', 'radio')` opens Settings at that section (`#settings-section-<param>`).
  $effect(() => {
    const section = nav.view === 'settings' ? nav.param : undefined;
    if (!section) return;
    void tick().then(() => {
      const reduce = typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;
      document
        .getElementById(`settings-section-${section}`)
        ?.scrollIntoView({ block: 'start', behavior: reduce ? 'auto' : 'smooth' });
    });
  });

  // Reload whenever the library changed (a scan finished, a folder came back online).
  $effect(() => {
    void library.version;
    void loadFolders();
  });

  $effect(() => {
    audioDevices()
      .then((names) => (devices = names ?? []))
      .catch((err) => console.error('listing the output devices failed', err));
    getVersion()
      .then((v) => (version = v))
      .catch(() => {});
  });

  async function add() {
    folderError = null;
    let path: string | string[] | null;
    try {
      path = await open({ directory: true, multiple: false, title: t('library.chooseFolder') });
    } catch (err) {
      folderError = t('library.addFailed', { message: String(err) });
      return;
    }
    if (typeof path !== 'string') return;
    adding = true;
    try {
      await libraryAddFolder(path);
      await Promise.all([reloadLibrary(), loadFolders()]);
    } catch (err) {
      folderError = t('library.addFailed', { message: String(err) });
    } finally {
      adding = false;
    }
  }

  /** Opens a folder's remove confirm with the focus on "Keep". */
  async function askRemove(path: string, event: MouseEvent) {
    const row = (event.currentTarget as HTMLElement).closest('li');
    confirming = path;
    await tick();
    row?.querySelector<HTMLElement>('.keep')?.focus();
  }

  /** Closes the confirm and hands the focus back to the folder's Remove. */
  async function keepFolder(event: Event) {
    const row = (event.currentTarget as HTMLElement).closest('li');
    confirming = null;
    await tick();
    row?.querySelector<HTMLElement>('.remove-trigger')?.focus();
  }

  function folderConfirmKeys(event: KeyboardEvent) {
    if (event.key !== 'Escape' || confirming === null) return;
    event.preventDefault();
    event.stopPropagation();
    void keepFolder(event);
  }

  async function remove(path: string) {
    confirming = null;
    folderError = null;
    try {
      await libraryRemoveFolder(path);
      await Promise.all([reloadLibrary(), loadFolders()]);
    } catch (err) {
      folderError = t('settings.folders.removeFailed', { message: String(err) });
    }
  }

  async function rescan() {
    folderError = null;
    rescanning = true;
    try {
      await libraryRescan();
      await loadFolders();
    } catch (err) {
      folderError = t('settings.folders.rescanFailed', { message: String(err) });
    } finally {
      rescanning = false;
    }
  }

  async function change(patch: Partial<Settings>) {
    saveError = null;
    try {
      await updateSettings(patch);
    } catch (err) {
      saveError = t('settings.saveFailed', { message: String(err) });
    }
  }

  /** The last part of a path, for the folder's title line. */
  function folderName(path: string): string {
    const parts = path.split(/[\\/]+/).filter(Boolean);
    return parts[parts.length - 1] ?? path;
  }

  const deviceOptions: SelectOption[] = $derived.by(() => {
    const options: SelectOption[] = [{ value: DEFAULT_DEVICE, label: t('settings.output.default') }];
    for (const name of devices) options.push({ value: name, label: name });
    const saved = settings.outputDevice;
    if (saved && !devices.includes(saved)) {
      options.push({ value: saved, label: saved, note: t('settings.output.missing') });
    }
    return options;
  });

  const languageOptions: SelectOption[] = $derived.by(() => {
    const detected = typeof navigator === 'undefined' ? 'en' : systemLang(navigator.languages ?? []);
    return [
      { value: 'system', label: t('settings.language.system', { name: t(`settings.language.${detected}`) }) },
      { value: 'en', label: t('settings.language.en') },
      { value: 'de', label: t('settings.language.de') },
    ];
  });

  const gainModes: ReplayGainMode[] = ['off', 'track', 'album'];
  const crossfadeModes: CrossfadeMode[] = ['off', 'flow', 'always'];

  // Crossfade length: 1 to 8 whole seconds on a 0..1 slider.
  const FADE_MIN_S = 1;
  const FADE_MAX_S = 8;
  const FADE_STEPS = FADE_MAX_S - FADE_MIN_S;
  /** The length shown while the slider is dragged, in seconds. */
  let fadeDrag = $state<number | null>(null);
  const fadeSeconds = $derived(fadeDrag ?? Math.round((settings.crossfadeMs ?? 4000) / 1000));
  const toFadeSeconds = (share: number) => FADE_MIN_S + Math.round(Math.min(Math.max(share, 0), 1) * FADE_STEPS);

  function commitFade(share: number) {
    fadeDrag = null;
    const ms = toFadeSeconds(share) * 1000;
    if (ms !== settings.crossfadeMs) void change({ crossfadeMs: ms });
  }

  let shortcutError = $state<string | null>(null);
  let systemError = $state<string | null>(null);

  /** The system may refuse the sign-in entry; then nothing is saved and the switch flips back. */
  async function changeStartWithSystem(on: boolean) {
    systemError = null;
    try {
      await updateSettings({ startWithSystem: on });
    } catch (err) {
      systemError = t('settings.startWithSystemFailed', { message: String(err) });
    }
  }

  // Navidrome: the sign-in form while no account is saved (or the password again for the
  // saved account), else the account with its sync status. The status store follows
  // `navidrome://status` only: a connect's own answer can be older than the events. The
  // password lives only in this field until a connect attempt, which clears it; the backend
  // hands it to the OS keychain.

  let ndUrl = $state('');
  let ndUser = $state('');
  let ndPassword = $state('');
  /** The saved account needs its password again (wrong credentials, keychain). Address and
   *  user stay fixed: another server or user (which wipes the library) goes through Disconnect. */
  let relogin = $state(false);
  let ndPending = $state<'test' | 'connect' | null>(null);
  let ndResult = $state<{ ok: boolean; text: string } | null>(null);
  let confirmDisconnect = $state(false);
  let disconnecting = $state(false);
  let ndActionError = $state<string | null>(null);
  /** The clock "Synced 4 minutes ago" is measured against, moved on twice a minute. */
  let now = $state(Date.now());
  let urlInput = $state<HTMLInputElement>();
  let passwordInput = $state<HTMLInputElement>();
  let accountActions = $state<HTMLElement>();
  let disconnectTrigger = $state<HTMLButtonElement>();
  let keepButton = $state<HTMLButtonElement>();

  const showForm = $derived(!navidrome.configured || relogin);
  /** The account a test or connect is for: the saved one when signing in again. */
  const formUrl = $derived(relogin ? (navidrome.url ?? '') : ndUrl.trim());
  const formUser = $derived(relogin ? (navidrome.user ?? '') : ndUser.trim());
  const ndFilled = $derived(formUrl !== '' && formUser !== '' && ndPassword !== '');
  const ndSyncing = $derived(navidrome.state === 'syncing');
  const syncProgress = $derived(navidrome.progress);
  const syncShare = $derived(
    syncProgress && syncProgress.total ? Math.min(1, syncProgress.done / syncProgress.total) : 0,
  );

  $effect(() => {
    const timer = setInterval(() => (now = Date.now()), 30_000);
    return () => clearInterval(timer);
  });

  // A disconnect from elsewhere (or a finished one) leaves no confirm row or sign-in behind.
  $effect(() => {
    if (!navidrome.configured) {
      confirmDisconnect = false;
      relogin = false;
    }
  });

  /** Any edit makes an earlier test result stale. */
  function edited() {
    ndResult = null;
  }

  function serverText(info: ServerInfo): string {
    const type = info.serverType?.trim();
    const server = type ? type.charAt(0).toUpperCase() + type.slice(1) : t('settings.navidrome.aServer');
    const version = info.serverVersion?.trim();
    return version
      ? t('settings.navidrome.testOk', { server, version, api: info.apiVersion })
      : t('settings.navidrome.testOkNoVersion', { server, api: info.apiVersion });
  }

  /** The first button of the account row ("Sync now" or "Sign in again"), once it is there. */
  async function focusAccount() {
    await tick();
    accountActions?.querySelector<HTMLElement>('button')?.focus();
  }

  // While a request runs the controls are read-only and `aria-disabled`, never `disabled`:
  // a disabled control drops the focus to the page.

  async function testConnection() {
    if (!ndFilled || ndPending) return;
    ndPending = 'test';
    ndResult = null;
    try {
      const info = await navidromeTest(formUrl, formUser, ndPassword);
      ndResult = { ok: true, text: serverText(info) };
    } catch (err) {
      ndResult = { ok: false, text: navidromeErrorText(err) };
    } finally {
      ndPending = null;
    }
  }

  async function connect(event?: SubmitEvent) {
    event?.preventDefault();
    if (!ndFilled || ndPending) return;
    ndPending = 'connect';
    ndResult = null;
    let ok = false;
    try {
      await navidromeConnect(formUrl, formUser, ndPassword);
      ok = true;
      relogin = false;
      ndActionError = null;
    } catch (err) {
      ndResult = { ok: false, text: navidromeErrorText(err) };
    } finally {
      // Cleared whatever the outcome: a failed attempt asks for it once more.
      ndPassword = '';
      ndPending = null;
    }
    if (ok) {
      await focusAccount();
    } else {
      await tick();
      passwordInput?.focus();
    }
  }

  async function signInAgain() {
    ndPassword = '';
    ndResult = null;
    confirmDisconnect = false;
    relogin = true;
    await tick();
    passwordInput?.focus();
  }

  async function cancelSignIn() {
    if (ndPending) return;
    ndPassword = '';
    ndResult = null;
    relogin = false;
    await focusAccount();
  }

  async function askDisconnect() {
    confirmDisconnect = true;
    await tick();
    keepButton?.focus();
  }

  async function keepAccount() {
    confirmDisconnect = false;
    await tick();
    disconnectTrigger?.focus();
  }

  function confirmKeys(event: KeyboardEvent) {
    if (event.key !== 'Escape') return;
    event.preventDefault();
    event.stopPropagation();
    void keepAccount();
  }

  async function syncNow() {
    if (ndSyncing) return;
    ndActionError = null;
    try {
      await navidromeSync();
    } catch (err) {
      ndActionError = t('settings.navidrome.syncFailed', { message: navidromeErrorText(err) });
    }
  }

  async function disconnect() {
    confirmDisconnect = false;
    disconnecting = true;
    ndActionError = null;
    try {
      await navidromeDisconnect();
      ndUrl = '';
      ndUser = '';
      ndPassword = '';
      ndResult = null;
      // The account row is gone with its buttons: the focus goes to the empty form.
      await tick();
      urlInput?.focus();
    } catch (err) {
      ndActionError = t('settings.navidrome.disconnectFailed', { message: navidromeErrorText(err) });
    } finally {
      disconnecting = false;
    }
    if (navidrome.configured) {
      await tick();
      disconnectTrigger?.focus();
    }
  }

  /** "4 minutes ago" in the UI language; under a minute "just now". */
  function ago(at: number, from: number): string {
    const seconds = Math.max(0, Math.round((from - at) / 1000));
    if (seconds < 60) return t('settings.navidrome.state.justNow');
    const format = new Intl.RelativeTimeFormat(lang(), { numeric: 'auto' });
    const minutes = Math.round(seconds / 60);
    if (minutes < 60) return format.format(-minutes, 'minute');
    const hours = Math.round(minutes / 60);
    if (hours < 24) return format.format(-hours, 'hour');
    return format.format(-Math.round(hours / 24), 'day');
  }

  /** What the live region says: one line per state (and failure), never the running counts,
   *  so a long sync is announced once, not on every page of songs. */
  const announcement = $derived.by(() => {
    if (!navidrome.configured) return '';
    switch (navidrome.state) {
      case 'syncing':
        return t('settings.navidrome.state.syncing');
      case 'online':
        return t('settings.navidrome.state.online');
      case 'offline':
        return t('settings.navidrome.state.offline');
      case 'error':
        return navidrome.error ? navidromeErrorText(navidrome.error) : '';
      default:
        return '';
    }
  });

  /** The status line's text; the failure of `error` is shown on its own. */
  const statusText = $derived.by(() => {
    switch (navidrome.state) {
      case 'syncing': {
        const p = syncProgress;
        if (p && p.total) {
          return t('settings.navidrome.state.syncingOf', {
            done: p.done.toLocaleString(lang()),
            total: p.total.toLocaleString(lang()),
          });
        }
        if (p && p.done > 0) return t('settings.navidrome.state.syncingSoFar', { done: p.done.toLocaleString(lang()) });
        return t('settings.navidrome.state.syncing');
      }
      case 'online':
        return navidrome.lastSync
          ? t('settings.navidrome.state.synced', { when: ago(navidrome.lastSync, now) })
          : t('settings.navidrome.state.online');
      case 'offline':
        return t('settings.navidrome.state.offline');
      default:
        return null;
    }
  });

  // While the song pass runs the status line carries the count itself.
  const ndCounts = $derived(
    !(ndSyncing && syncProgress) && (navidrome.trackCount > 0 || navidrome.albumCount > 0)
      ? `${tc('view.count.tracks', navidrome.trackCount)}, ${tc('view.count.albums', navidrome.albumCount)}`
      : null,
  );

  const qualities: StreamQuality[] = ['original', '320', '192', '128'];
  const qualityOptions = $derived(
    qualities.map((q) => ({
      value: q,
      label: q === 'original' ? t('settings.navidrome.quality.original') : q,
    })),
  );

  // Radio location. Opening Settings never asks the location service: only "Automatic" and
  // "Detect again" do, and the Radio view when it first needs the place.
  const radioLocation = $derived(settings.radioLocation ?? { mode: 'auto' });
  const isManual = $derived(radioLocation.mode === 'manual');
  const manualCountry = $derived(radioLocation.mode === 'manual' ? radioLocation.countryCode : null);
  const manualRegion = $derived(radioLocation.mode === 'manual' ? radioLocation.region : null);
  let countries = $state.raw<RadioCountry[]>([]);
  let regions = $state.raw<RadioRegion[]>([]);
  let detecting = $state(false);

  function countryName(code: string): string {
    try {
      return new Intl.DisplayNames([lang()], { type: 'region' }).of(code) ?? code;
    } catch {
      return code;
    }
  }

  const placeText = $derived.by(() => {
    const place = radio.place;
    if (!place) return null;
    return [place.city, countryName(place.countryCode)].filter(Boolean).join(', ');
  });

  $effect(() => {
    if (!isManual) return;
    let gone = false;
    listCountries()
      .then((list) => {
        if (!gone) countries = list;
      })
      .catch((err) => console.error('listing the radio countries failed', err));
    return () => {
      gone = true;
    };
  });

  $effect(() => {
    const code = manualCountry;
    regions = [];
    if (code === null) return;
    let gone = false;
    listRegions(code)
      .then((list) => {
        if (!gone) regions = list;
      })
      .catch((err) => console.error('listing the radio regions failed', err));
    return () => {
      gone = true;
    };
  });

  // The saved choice stays selectable while the directory cannot list it.
  const countryOptions: SelectOption[] = $derived.by(() => {
    const list = countries.map((c) => ({ value: c.code, label: countryName(c.code) }));
    if (manualCountry && !list.some((c) => c.value === manualCountry)) {
      list.push({ value: manualCountry, label: countryName(manualCountry) });
    }
    return list.sort((a, b) => a.label.localeCompare(b.label, lang()));
  });
  const regionOptions: SelectOption[] = $derived.by(() => {
    const list = [
      { value: '', label: t('settings.radio.anyRegion') },
      ...regions.map((r) => ({ value: r.name, label: r.name })),
    ];
    if (manualRegion && !list.some((r) => r.value === manualRegion)) list.push({ value: manualRegion, label: manualRegion });
    return list;
  });

  async function changeLocation(next: RadioLocation) {
    await change({ radioLocation: next });
    if (saveError === null) void loadNear();
  }

  function pickLocationMode(mode: 'auto' | 'manual') {
    if (mode === radioLocation.mode) return;
    if (mode === 'auto') return void changeLocation({ mode: 'auto' });
    const countryCode = radio.place?.countryCode ?? (lang() === 'de' ? 'DE' : 'US');
    void changeLocation({ mode: 'manual', countryCode, region: null });
  }

  async function detectAgain() {
    detecting = true;
    try {
      await redetectPlace();
    } finally {
      detecting = false;
    }
  }

  async function changeShortcut(accelerator: string) {
    shortcutError = null;
    try {
      await updateSettings({ miniShortcut: accelerator });
    } catch (err) {
      // The backend says whether the keys cannot be a shortcut or another app holds them.
      const message = String(err);
      const keys = describeShortcut(accelerator, IS_MAC);
      shortcutError = message.startsWith('invalid shortcut')
        ? t('settings.shortcut.invalid')
        : message.startsWith('shortcut taken')
          ? t('settings.shortcut.taken', { keys })
          : t('settings.saveFailed', { message });
    }
  }
</script>

<div class="settings">
  <div class="inner">
    <ViewHead title={t('nav.settings')} />

    <section aria-labelledby="settings-library">
      <h2 id="settings-library">{t('settings.library')}</h2>
      <p class="lead">{t('settings.libraryLead')}</p>

      <div class="panel">
        {#if folders && folders.length > 0}
          <ul class="folders">
            {#each folders as folder (folder.path)}
              <li class="folder" class:offline={!folder.online}>
                <span class="folder-icon"><Icon name="folder" size={20} /></span>
                <div class="folder-text">
                  <strong>{folderName(folder.path)}</strong>
                  <span class="path">{folder.path}</span>
                  {#if !folder.online}<span class="offline-hint">{t('settings.folders.offlineHint')}</span>{/if}
                </div>
                <span class="state" class:on={folder.online}>
                  <i aria-hidden="true"></i>{folder.online ? t('settings.folders.online') : t('settings.folders.offline')}
                </span>
                <span class="count">{tc('view.count.tracks', folder.trackCount)}</span>
                <!-- svelte-ignore a11y_no_static_element_interactions -->
                <div class="folder-actions" onkeydown={folderConfirmKeys}>
                  {#if confirming === folder.path}
                    <button type="button" class="link danger" onclick={() => remove(folder.path)}>
                      {t('settings.folders.confirmRemove')}
                    </button>
                    <button type="button" class="link keep" onclick={keepFolder}>{t('settings.folders.cancel')}</button>
                  {:else}
                    <button
                      type="button"
                      class="link remove-trigger"
                      aria-label={t('settings.folders.removeLabel', { path: folder.path })}
                      disabled={busy}
                      onclick={(event) => askRemove(folder.path, event)}
                    >
                      {t('settings.folders.remove')}
                    </button>
                  {/if}
                </div>
              </li>
            {/each}
          </ul>
        {:else if folders}
          <p class="empty">{t('settings.folders.empty')}</p>
        {/if}

        <div class="folder-bar">
          <button type="button" class="btn ghost small" disabled={busy} onclick={add}>
            <Icon name="folderAdd" size={17} />
            {t('library.addFolder')}
          </button>
          {#if folders && folders.length > 0}
            <button type="button" class="btn ghost small" disabled={busy} onclick={rescan}>
              <Icon name="refresh" size={17} />
              {t('settings.folders.rescan')}
            </button>
          {/if}
          <div class="progress" class:shown={busy} role="status" aria-live="polite">
            {#if busy}
              <div class="bar" class:indeterminate={!scan || scan.total === 0}>
                <i style={scan && scan.total > 0 ? `transform:scaleX(${share})` : undefined}></i>
              </div>
              <span>
                {scan && scan.total > 0
                  ? t('library.scanning', { done: scan.done.toLocaleString(lang()), total: scan.total.toLocaleString(lang()) })
                  : t('library.looking')}
              </span>
            {/if}
          </div>
        </div>
      </div>
      {#if folderError}<p class="error" role="alert">{folderError}</p>{/if}
    </section>

    <section aria-labelledby="settings-navidrome">
      <h2 id="settings-navidrome">{t('settings.navidrome')}</h2>
      <p class="lead">{t('settings.navidrome.lead')}</p>
      <span class="sr-only nd-live" role="status" aria-live="polite">{announcement}</span>

      <div class="panel">
        {#if showForm}
          <form class="nd-form" onsubmit={connect} aria-busy={ndPending !== null}>
            {#if relogin}
              <div class="relogin wide">
                <strong class="server">{navidrome.url}</strong>
                <span class="hint">{t('settings.navidrome.signedInAs', { user: navidrome.user ?? '' })}</span>
              </div>
            {:else}
              <label class="field wide">
                <span class="field-label">{t('settings.navidrome.url')}</span>
                <input
                  bind:this={urlInput}
                  name="navidrome-url"
                  type="text"
                  inputmode="url"
                  placeholder="https://music.example.com"
                  autocomplete="off"
                  autocapitalize="off"
                  spellcheck="false"
                  bind:value={ndUrl}
                  oninput={edited}
                  readonly={ndPending !== null}
                  aria-disabled={ndPending !== null}
                />
              </label>
              <label class="field">
                <span class="field-label">{t('settings.navidrome.user')}</span>
                <input
                  name="navidrome-user"
                  type="text"
                  autocomplete="off"
                  autocapitalize="off"
                  spellcheck="false"
                  bind:value={ndUser}
                  oninput={edited}
                  readonly={ndPending !== null}
                  aria-disabled={ndPending !== null}
                />
              </label>
            {/if}
            <label class="field">
              <span class="field-label">{t('settings.navidrome.password')}</span>
              <input
                bind:this={passwordInput}
                name="navidrome-password"
                type="password"
                autocomplete="new-password"
                bind:value={ndPassword}
                oninput={edited}
                readonly={ndPending !== null}
                aria-disabled={ndPending !== null}
              />
            </label>

            <div class="nd-actions">
              <button
                type="submit"
                class="btn primary small"
                disabled={!ndFilled && ndPending === null}
                aria-disabled={ndPending !== null}
              >
                {t('settings.navidrome.connect')}
              </button>
              <button
                type="button"
                class="btn ghost small"
                disabled={!ndFilled && ndPending === null}
                aria-disabled={ndPending !== null}
                onclick={testConnection}
              >
                {t('settings.navidrome.test')}
              </button>
              {#if relogin}
                <button type="button" class="link" aria-disabled={ndPending !== null} onclick={cancelSignIn}>
                  {t('settings.navidrome.cancel')}
                </button>
              {/if}
              <div class="nd-feedback" role="status" aria-live="polite">
                {#if ndPending}
                  <span class="pending">
                    <span class="spinner" aria-hidden="true"></span>
                    {t(ndPending === 'test' ? 'settings.navidrome.testing' : 'settings.navidrome.connecting')}
                  </span>
                {:else if ndResult?.ok}
                  <span class="result ok"><Icon name="check" size={16} />{ndResult.text}</span>
                {/if}
              </div>
            </div>
            {#if ndResult && !ndResult.ok && !ndPending}
              <p class="result fail" role="alert">{ndResult.text}</p>
            {/if}
          </form>
        {:else}
          <div class="account">
            <span class="dot {navidrome.state}" aria-hidden="true"></span>
            <div class="account-text">
              <strong class="server">{navidrome.url}</strong>
              <span class="hint">{t('settings.navidrome.signedInAs', { user: navidrome.user ?? '' })}</span>
            </div>
            <div class="account-actions" bind:this={accountActions}>
              {#if navidrome.state === 'error' && navidrome.error?.code !== 'syncRefused'}
                <button type="button" class="btn ghost small" disabled={confirmDisconnect || disconnecting} onclick={signInAgain}>
                  {t('settings.navidrome.signInAgain')}
                </button>
              {:else}
                <!-- Busy while syncing, but still focusable: a connect hands the focus here. -->
                <button
                  type="button"
                  class="btn ghost small"
                  disabled={confirmDisconnect || disconnecting}
                  aria-disabled={ndSyncing}
                  onclick={syncNow}
                >
                  <Icon name="refresh" size={17} />
                  {t('settings.navidrome.syncNow')}
                </button>
              {/if}
              {#if !confirmDisconnect}
                <button
                  bind:this={disconnectTrigger}
                  type="button"
                  class="link"
                  aria-disabled={disconnecting}
                  onclick={() => !disconnecting && askDisconnect()}
                >
                  {t('settings.navidrome.disconnect')}
                </button>
              {/if}
            </div>
          </div>

          {#if confirmDisconnect}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="confirm-row" onkeydown={confirmKeys}>
              <span>{t('settings.navidrome.confirmDisconnect')}</span>
              <div class="confirm-actions">
                <button type="button" class="link danger" onclick={disconnect}>{t('settings.navidrome.disconnect')}</button>
                <button bind:this={keepButton} type="button" class="link keep" onclick={keepAccount}>
                  {t('settings.navidrome.keep')}
                </button>
              </div>
            </div>
          {/if}

          <div class="status">
            {#if navidrome.state === 'error' && navidrome.error}
              <p class="status-error">{navidromeErrorText(navidrome.error)}</p>
            {/if}
            {#if statusText || ndCounts}
              <div class="status-line">
                {#if statusText}<span class="status-text">{statusText}</span>{/if}
                {#if ndCounts}<span class="status-counts">{ndCounts}</span>{/if}
              </div>
            {/if}
            {#if ndSyncing}
              <div
                class="bar wide"
                class:indeterminate={!syncProgress?.total}
                role="progressbar"
                aria-label={t('settings.navidrome.state.syncing')}
                aria-valuemin={0}
                aria-valuemax={syncProgress?.total ?? undefined}
                aria-valuenow={syncProgress?.total ? syncProgress.done : undefined}
              >
                <i style={syncProgress?.total ? `transform:scaleX(${syncShare})` : undefined}></i>
              </div>
            {/if}
            <span class="keychain">
              {t(IS_MAC ? 'settings.navidrome.keychainMac' : 'settings.navidrome.keychainWindows')}
            </span>
          </div>
        {/if}

        <div class="row">
          <div class="row-text">
            <span class="row-label" id="label-quality">{t('settings.navidrome.quality')}</span>
            <span class="hint">{t('settings.navidrome.qualityHint')}</span>
          </div>
          <div class="control">
            <Segmented
              labelledby="label-quality"
              attr="data-quality"
              value={settings.navidromeQuality ?? 'original'}
              options={qualityOptions}
              onpick={(next) => change({ navidromeQuality: next })}
            />
          </div>
        </div>
      </div>
      {#if ndActionError}<p class="error" role="alert">{ndActionError}</p>{/if}
    </section>

    <section id="settings-section-radio" aria-labelledby="settings-radio">
      <h2 id="settings-radio">{t('settings.radio')}</h2>
      <div class="panel">
        <div class="row">
          <div class="row-text">
            <span class="row-label" id="label-radio-location">{t('settings.radio.location')}</span>
          </div>
          <div class="control">
            <Segmented
              labelledby="label-radio-location"
              attr="data-radio-location"
              value={radioLocation.mode}
              options={[
                { value: 'auto', label: t('settings.radio.auto') },
                { value: 'manual', label: t('settings.radio.manual') },
              ]}
              onpick={pickLocationMode}
            />
          </div>
        </div>

        {#if radioLocation.mode === 'auto'}
          <div class="row">
            <div class="row-text">
              <span class="row-label">
                {placeText ? t('settings.radio.detected', { place: placeText }) : t('settings.radio.notFound')}
              </span>
              <span class="hint">{t('settings.radio.privacy')}</span>
            </div>
            <div class="control end">
              <button type="button" class="btn ghost small" disabled={detecting} onclick={detectAgain}>
                <Icon name="refresh" size={17} />
                {t('settings.radio.redetect')}
              </button>
            </div>
          </div>
        {:else}
          <div class="row">
            <div class="row-text">
              <span class="row-label">{t('settings.radio.country')}</span>
            </div>
            <div class="control">
              <Select
                label={t('settings.radio.country')}
                value={manualCountry ?? ''}
                options={countryOptions}
                onchange={(code) => changeLocation({ mode: 'manual', countryCode: code, region: null })}
              />
            </div>
          </div>
          <div class="row">
            <div class="row-text">
              <span class="row-label">{t('settings.radio.region')}</span>
            </div>
            <div class="control">
              <Select
                label={t('settings.radio.region')}
                value={manualRegion ?? ''}
                options={regionOptions}
                onchange={(region) =>
                  changeLocation({ mode: 'manual', countryCode: manualCountry ?? '', region: region || null })}
              />
            </div>
          </div>
        {/if}
      </div>
    </section>

    <section aria-labelledby="settings-playback">
      <h2 id="settings-playback">{t('settings.playback')}</h2>
      <div class="panel">
        <div class="row">
          <div class="row-text">
            <span class="row-label" id="label-output">{t('settings.output')}</span>
            <span class="hint">{t('settings.outputHint')}</span>
          </div>
          <div class="control">
            <Select
              label={t('settings.output')}
              value={settings.outputDevice ?? DEFAULT_DEVICE}
              options={deviceOptions}
              onchange={(value) => change({ outputDevice: value === DEFAULT_DEVICE ? null : value })}
            />
          </div>
        </div>

        <div class="row">
          <div class="row-text">
            <span class="row-label" id="label-gapless">{t('settings.gapless')}</span>
            <span class="hint">{t('settings.gaplessHint')}</span>
          </div>
          <div class="control end">
            <button
              type="button"
              class="switch"
              role="switch"
              aria-checked={settings.gapless}
              aria-labelledby="label-gapless"
              onclick={() => change({ gapless: !settings.gapless })}
            >
              <i></i>
            </button>
          </div>
        </div>

        <div class="row">
          <div class="row-text">
            <span class="row-label" id="label-gain">{t('settings.replayGain')}</span>
            <span class="hint">{t('settings.replayGainHint')}</span>
          </div>
          <div class="control">
            <Segmented
              labelledby="label-gain"
              attr="data-mode"
              value={settings.replayGain}
              options={gainModes.map((mode) => ({ value: mode, label: t(`settings.replayGain.${mode}`) }))}
              onpick={(next) => change({ replayGain: next })}
            />
          </div>
        </div>

        <div class="row">
          <div class="row-text">
            <span class="row-label" id="label-crossfade">{t('settings.crossfade')}</span>
            <span class="hint">{t('settings.crossfadeHint')}</span>
          </div>
          <div class="control">
            <Segmented
              labelledby="label-crossfade"
              attr="data-crossfade"
              value={settings.crossfade}
              options={crossfadeModes.map((mode) => ({ value: mode, label: t(`settings.crossfade.${mode}`) }))}
              onpick={(next) => change({ crossfade: next })}
            />
          </div>
        </div>

        <div class="row">
          <div class="row-text">
            <span class="row-label">{t('settings.crossfadeLength')}</span>
            <span class="hint">{t('settings.crossfadeLengthHint')}</span>
          </div>
          <div class="control length">
            <Slider
              label={t('settings.crossfadeLength')}
              value={(fadeSeconds - FADE_MIN_S) / FADE_STEPS}
              step={1 / FADE_STEPS}
              valueText={t('settings.seconds', { count: fadeSeconds })}
              disabled={settings.crossfade === 'off'}
              oninput={(share) => (fadeDrag = toFadeSeconds(share))}
              oncommit={commitFade}
              oncancel={() => (fadeDrag = null)}
            />
            <span class="length-value" aria-hidden="true">{t('settings.seconds', { count: fadeSeconds })}</span>
          </div>
        </div>
      </div>
    </section>

    <section aria-labelledby="settings-mini">
      <h2 id="settings-mini">{t('settings.mini')}</h2>
      <div class="panel">
        <div class="row">
          <div class="row-text">
            <span class="row-label" id="label-on-top">{t('settings.miniOnTop')}</span>
            <span class="hint">{t('settings.miniOnTopHint')}</span>
          </div>
          <div class="control end">
            <button
              type="button"
              class="switch"
              role="switch"
              aria-checked={settings.miniOnTop}
              aria-labelledby="label-on-top"
              onclick={() => change({ miniOnTop: !settings.miniOnTop })}
            >
              <i></i>
            </button>
          </div>
        </div>

        <div class="row">
          <div class="row-text">
            <span class="row-label">{t('settings.shortcut')}</span>
            <span class="hint">{t('settings.shortcutHint')}</span>
          </div>
          <div class="control">
            <ShortcutInput
              value={settings.miniShortcut}
              onchange={changeShortcut}
              describedby={shortcutError ? 'shortcut-error' : undefined}
            />
          </div>
        </div>
      </div>
      {#if shortcutError}<p class="error" id="shortcut-error" role="alert">{shortcutError}</p>{/if}
    </section>

    <section aria-labelledby="settings-covers">
      <h2 id="settings-covers">{t('settings.covers')}</h2>
      <div class="panel">
        <div class="row">
          <div class="row-text">
            <span class="row-label" id="label-covers">{t('settings.coverLookup')}</span>
            <span class="hint">{t('settings.coverLookupHint')}</span>
          </div>
          <div class="control end">
            <button
              type="button"
              class="switch"
              role="switch"
              aria-checked={settings.coverLookup}
              aria-labelledby="label-covers"
              onclick={() => change({ coverLookup: !settings.coverLookup })}
            >
              <i></i>
            </button>
          </div>
        </div>
      </div>
    </section>

    <section aria-labelledby="settings-system">
      <h2 id="settings-system">{t('settings.system')}</h2>
      <div class="panel">
        <div class="row">
          <div class="row-text">
            <span class="row-label" id="label-autostart">{t('settings.startWithSystem')}</span>
            <span class="hint">{t(IS_MAC ? 'settings.startWithSystemHintMac' : 'settings.startWithSystemHint')}</span>
          </div>
          <div class="control end">
            <button
              type="button"
              class="switch"
              role="switch"
              aria-checked={settings.startWithSystem}
              aria-labelledby="label-autostart"
              onclick={() => changeStartWithSystem(!settings.startWithSystem)}
            >
              <i></i>
            </button>
          </div>
        </div>
      </div>
      {#if systemError}<p class="error" role="alert">{systemError}</p>{/if}
    </section>

    <section aria-labelledby="settings-language">
      <h2 id="settings-language">{t('settings.language')}</h2>
      <div class="panel">
        <div class="row">
          <div class="row-text">
            <span class="row-label">{t('settings.languageLabel')}</span>
            <span class="hint">{t('settings.languageHint')}</span>
          </div>
          <div class="control">
            <Select
              label={t('settings.languageLabel')}
              value={settings.language}
              options={languageOptions}
              onchange={(value) => change({ language: value as LanguagePref })}
            />
          </div>
        </div>
      </div>
      {#if saveError}<p class="error" role="alert">{saveError}</p>{/if}
    </section>

    <section class="about" aria-label={t('app.about')}>
      <img src={logoUrl} alt="" width="40" height="40" draggable="false" />
      <div class="about-text">
        <strong>Prism{version ? ` ${version}` : ''}</strong>
        <span>{t('about.tagline')}</span>
      </div>
      <button type="button" class="btn ghost small" onclick={() => (overlays.about = true)}>{t('app.about')}</button>
    </section>
  </div>
</div>

<style>
  .settings {
    height: 100%;
    overflow-x: hidden;
    overflow-y: auto;
    scrollbar-color: auto;
  }

  .settings::-webkit-scrollbar {
    width: 6px;
    height: 0;
  }

  .settings::-webkit-scrollbar-thumb {
    border: 0;
    border-radius: 3px;
    background: rgba(255, 255, 255, 0.14);
  }

  .inner {
    max-width: 760px;
    padding: 4px 32px 56px;
  }

  section + section {
    margin-top: 34px;
  }

  h2 {
    margin: 0 0 4px;
    font-family: var(--font-display);
    font-size: 18px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .lead {
    margin: 0;
    max-width: 62ch;
    font-size: 13.5px;
    line-height: 1.55;
    color: var(--text-dim);
  }

  .panel {
    margin-top: 14px;
    border-radius: 14px;
    background: rgba(255, 255, 255, 0.028);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.06);
  }

  /* Library */

  .folders {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .folder {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto auto auto;
    align-items: center;
    gap: 14px;
    padding: 14px 16px;
    border-bottom: 1px solid var(--line);
  }

  .folder-icon {
    display: grid;
    color: var(--text-dim);
  }

  .folder-text {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .folder-text strong {
    font-size: 14px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .path {
    font-size: 12.5px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    user-select: text;
    -webkit-user-select: text;
  }

  .offline-hint {
    font-size: 12px;
    color: rgba(243, 243, 245, 0.5);
  }

  .offline .folder-text strong,
  .offline .folder-icon {
    opacity: 0.6;
  }

  .state {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    font-size: 12.5px;
    color: var(--text-dim);
    white-space: nowrap;
  }

  .state i {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--spec-1);
  }

  .state.on i {
    background: var(--spec-3);
    box-shadow: 0 0 8px color-mix(in srgb, var(--spec-3) 60%, transparent);
  }

  .count {
    min-width: 72px;
    text-align: right;
    font-size: 12.5px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .folder-actions {
    display: flex;
    gap: 4px;
  }

  .link {
    height: 30px;
    padding: 0 10px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    color: var(--text-dim);
    font: inherit;
    font-size: 12.5px;
    font-weight: 500;
    white-space: nowrap;
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out;
  }

  .link:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.07);
    color: var(--text);
  }

  .link.danger {
    background: color-mix(in srgb, var(--spec-1) 16%, transparent);
    color: #ff8fa3;
  }

  .link.danger:hover {
    background: color-mix(in srgb, var(--spec-1) 24%, transparent);
    color: #ffb3c0;
  }

  .link:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 1px;
  }

  .link:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .empty {
    margin: 0;
    padding: 18px 16px;
    font-size: 13.5px;
    color: var(--text-dim);
    border-bottom: 1px solid var(--line);
  }

  .folder-bar {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    padding: 12px 16px;
  }

  :global(.btn.small) {
    height: 32px;
    padding: 0 14px 0 11px;
    font-size: 12.5px;
  }

  .progress {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 12.5px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
    opacity: 0;
    transition: opacity var(--dur) ease-out;
  }

  .progress.shown {
    opacity: 1;
  }

  .bar {
    position: relative;
    width: 140px;
    height: 3px;
    border-radius: 2px;
    overflow: hidden;
    background: rgba(255, 255, 255, 0.08);
  }

  .bar i {
    position: absolute;
    inset: 0;
    transform-origin: left;
    background: linear-gradient(90deg, var(--spec-1), var(--spec-2), var(--spec-3), var(--spec-4));
    transition: transform var(--dur) linear;
  }

  .bar.indeterminate i {
    width: 40%;
    animation: sweep 1.2s ease-in-out infinite;
  }

  /* Navidrome: the sign-in form, or the account with its status line. */

  .nd-form {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    gap: 14px;
    padding: 16px;
  }

  .field {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .field.wide,
  .nd-actions,
  .result.fail {
    grid-column: 1 / -1;
  }

  .field-label {
    font-size: 12.5px;
    font-weight: 500;
    color: var(--text-dim);
  }

  .field input {
    height: 36px;
    min-width: 0;
    padding: 0 12px;
    border: 0;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.055);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08);
    color: var(--text);
    font: inherit;
    font-size: 13.5px;
    outline: none;
    transition:
      background-color var(--dur) ease-out,
      box-shadow var(--dur) ease-out;
  }

  .field input::placeholder {
    color: rgba(243, 243, 245, 0.32);
  }

  .field input:hover:not([aria-disabled='true']) {
    background: rgba(255, 255, 255, 0.075);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.12);
  }

  .field input:focus {
    background: rgba(255, 255, 255, 0.075);
    box-shadow:
      inset 0 0 0 1px color-mix(in srgb, var(--src-navidrome) 55%, transparent),
      0 0 0 3px color-mix(in srgb, var(--src-navidrome) 12%, transparent);
  }

  .nd-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    padding-top: 2px;
  }

  .nd-feedback {
    min-width: 0;
    margin-left: 6px;
    font-size: 12.5px;
    color: var(--text-dim);
  }

  .pending,
  .result.ok {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }

  .result.ok {
    color: var(--src-navidrome);
  }

  .result.fail {
    margin: 0;
    font-size: 13px;
    line-height: 1.5;
    color: #ff8fa3;
  }

  .spinner {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 2px solid rgba(255, 255, 255, 0.14);
    border-top-color: var(--src-navidrome);
    animation: spin 0.8s linear infinite;
  }

  .account {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 14px;
    padding: 16px 16px 10px;
  }

  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--src-navidrome);
    transition:
      background-color var(--dur) ease-out,
      box-shadow var(--dur) ease-out;
  }

  .dot.online {
    box-shadow: 0 0 9px color-mix(in srgb, var(--src-navidrome) 60%, transparent);
  }

  .dot.syncing {
    animation: breathe 1.6s ease-in-out infinite;
  }

  /* Offline: the source color as a hollow ring, the library still there but quiet. */
  .dot.offline {
    background: transparent;
    box-shadow: inset 0 0 0 1.5px var(--src-navidrome);
  }

  .dot.error {
    background: var(--spec-1);
    box-shadow: 0 0 9px color-mix(in srgb, var(--spec-1) 55%, transparent);
  }

  .account-text {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .server {
    font-size: 14px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    user-select: text;
    -webkit-user-select: text;
  }

  .account-actions,
  .confirm-actions {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .confirm-row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: 8px 14px;
    margin: 2px 16px 6px 39px;
    padding: 8px 6px 8px 12px;
    border-radius: 10px;
    background: color-mix(in srgb, var(--spec-1) 7%, transparent);
    font-size: 13px;
  }

  /* Indented under the server name: dot (9) plus the gap (14). */
  .status {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 0 16px 16px 39px;
  }

  .status-line {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 0;
    font-size: 13px;
    font-variant-numeric: tabular-nums;
  }

  .status-counts {
    color: var(--text-dim);
  }

  .status-text + .status-counts::before {
    content: '\00b7';
    margin: 0 8px;
    color: rgba(243, 243, 245, 0.35);
  }

  .status-error {
    margin: 0;
    max-width: 62ch;
    font-size: 13px;
    line-height: 1.5;
    color: #ff8fa3;
  }

  .bar.wide {
    width: min(100%, 320px);
  }

  .keychain {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-dim);
  }

  /* Signing in again: the saved account as text above the password. */
  .relogin {
    grid-column: 1 / -1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  /* Busy controls stay focusable: dimmed like disabled ones, but not `disabled`. */
  .panel :global(.btn[aria-disabled='true']),
  .link[aria-disabled='true'] {
    opacity: 0.5;
    cursor: default;
  }

  .field input[aria-disabled='true'] {
    opacity: 0.55;
  }

  .nd-form + .row,
  .status + .row {
    border-top: 1px solid var(--line);
  }

  /* Rows: a label with its hint on the left, the control on the right. */

  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 270px;
    align-items: center;
    gap: 28px;
    padding: 16px;
  }

  .row + .row {
    border-top: 1px solid var(--line);
  }

  .row-text {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .row-label {
    font-size: 14px;
    font-weight: 550;
  }

  .hint {
    font-size: 12.5px;
    line-height: 1.5;
    color: var(--text-dim);
    text-wrap: pretty;
  }

  .control.end {
    display: flex;
    justify-content: flex-end;
  }

  .control.length {
    display: flex;
    align-items: center;
    gap: 14px;
  }

  .control.length > :global(.slider) {
    flex: 1;
  }

  .length-value {
    min-width: 30px;
    text-align: right;
    font-size: 13px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }

  .switch {
    position: relative;
    width: 42px;
    height: 24px;
    padding: 0;
    border: 0;
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.12);
    cursor: pointer;
    transition: background-color var(--dur) ease-out;
  }

  .switch i {
    position: absolute;
    top: 3px;
    left: 3px;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--text);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.4);
    transition: transform var(--dur) ease-out;
  }

  .switch[aria-checked='true'] {
    background: var(--spec-3);
  }

  .switch[aria-checked='true'] i {
    transform: translateX(18px);
    background: var(--bg);
  }

  .switch:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 2px;
  }

  .error {
    margin: 10px 0 0;
    font-size: 13px;
    color: #ff8fa3;
  }

  /* About: a quiet line at the end, the logo beside the version. */

  .about {
    margin-top: 40px;
    display: flex;
    align-items: center;
    gap: 14px;
    padding-top: 22px;
    border-top: 1px solid var(--line);
  }

  .about img {
    border-radius: 10px;
    box-shadow:
      0 0 0 1px rgba(255, 255, 255, 0.08),
      0 6px 16px rgba(0, 0, 0, 0.45);
  }

  .about-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-right: auto;
  }

  .about-text strong {
    font-family: var(--font-display);
    font-size: 15px;
    font-weight: 600;
  }

  .about-text span {
    font-size: 12.5px;
    color: var(--text-dim);
  }

  @keyframes sweep {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(260%);
    }
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @keyframes breathe {
    50% {
      opacity: 0.35;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .bar.indeterminate i {
      animation: none;
      width: 100%;
      opacity: 0.5;
    }

    /* A fade, not motion: it keeps running, only slower. */
    .dot.syncing {
      animation-duration: 2.4s;
    }

    /* No turning: the ring stays put and only fades. */
    .spinner {
      border-color: color-mix(in srgb, var(--src-navidrome) 70%, transparent);
      animation: breathe 1.6s ease-in-out infinite;
    }
  }

  @media (max-width: 640px) {
    .nd-form {
      grid-template-columns: minmax(0, 1fr);
    }

    .account {
      grid-template-columns: auto minmax(0, 1fr);
    }

    .account-actions {
      grid-column: 2;
    }
  }

  @media (max-width: 980px) {
    .row {
      grid-template-columns: minmax(0, 1fr);
      gap: 12px;
    }

    .folder {
      grid-template-columns: auto minmax(0, 1fr) auto;
    }

    .count {
      display: none;
    }
  }
</style>
