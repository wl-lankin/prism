<script lang="ts">
  import { getVersion } from '@tauri-apps/api/app';
  import { open } from '@tauri-apps/plugin-dialog';
  import logoUrl from '../../../assets/logo.svg';
  import { lang, systemLang, t, tc } from '../i18n';
  import { audioDevices, libraryAddFolder, libraryFolders, libraryRemoveFolder, libraryRescan } from '../ipc';
  import { library, reloadLibrary } from '../stores/library.svelte';
  import { overlays } from '../stores/nav.svelte';
  import { settings, updateSettings } from '../stores/settings.svelte';
  import type { CrossfadeMode, Folder, LanguagePref, ReplayGainMode, Settings } from '../types';
  import Icon from './Icon.svelte';
  import Select, { type SelectOption } from './Select.svelte';
  import ShortcutInput, { IS_MAC, describeShortcut } from './ShortcutInput.svelte';
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

  /** Arrow keys in a segmented radio group: pick the neighbour (wrapping) and focus it. */
  function onsegmentkey<T extends string>(event: KeyboardEvent, modes: T[], index: number, attribute: string, pick: (mode: T) => void) {
    const step = event.key === 'ArrowRight' || event.key === 'ArrowDown' ? 1 : event.key === 'ArrowLeft' || event.key === 'ArrowUp' ? -1 : 0;
    if (!step) return;
    event.preventDefault();
    const next = modes[(index + step + modes.length) % modes.length];
    pick(next);
    const group = (event.currentTarget as HTMLElement).parentElement;
    queueMicrotask(() => group?.querySelector<HTMLElement>(`[${attribute}="${next}"]`)?.focus());
  }

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
                <div class="folder-actions">
                  {#if confirming === folder.path}
                    <button type="button" class="link danger" onclick={() => remove(folder.path)}>
                      {t('settings.folders.confirmRemove')}
                    </button>
                    <button type="button" class="link" onclick={() => (confirming = null)}>{t('settings.folders.cancel')}</button>
                  {:else}
                    <button
                      type="button"
                      class="link"
                      aria-label={t('settings.folders.removeLabel', { path: folder.path })}
                      disabled={busy}
                      onclick={() => (confirming = folder.path)}
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
            <div class="segments" role="radiogroup" aria-labelledby="label-gain">
              {#each gainModes as mode, index (mode)}
                {@const on = settings.replayGain === mode}
                <button
                  type="button"
                  role="radio"
                  data-mode={mode}
                  aria-checked={on}
                  tabindex={on ? 0 : -1}
                  class:on
                  onclick={() => change({ replayGain: mode })}
                  onkeydown={(event) => onsegmentkey(event, gainModes, index, 'data-mode', (next) => change({ replayGain: next }))}
                >
                  {t(`settings.replayGain.${mode}`)}
                </button>
              {/each}
            </div>
          </div>
        </div>

        <div class="row">
          <div class="row-text">
            <span class="row-label" id="label-crossfade">{t('settings.crossfade')}</span>
            <span class="hint">{t('settings.crossfadeHint')}</span>
          </div>
          <div class="control">
            <div class="segments" role="radiogroup" aria-labelledby="label-crossfade">
              {#each crossfadeModes as mode, index (mode)}
                {@const on = settings.crossfade === mode}
                <button
                  type="button"
                  role="radio"
                  data-crossfade={mode}
                  aria-checked={on}
                  tabindex={on ? 0 : -1}
                  class:on
                  onclick={() => change({ crossfade: mode })}
                  onkeydown={(event) => onsegmentkey(event, crossfadeModes, index, 'data-crossfade', (next) => change({ crossfade: next }))}
                >
                  {t(`settings.crossfade.${mode}`)}
                </button>
              {/each}
            </div>
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
            <ShortcutInput value={settings.miniShortcut} onchange={changeShortcut} />
          </div>
        </div>
      </div>
      {#if shortcutError}<p class="error" role="alert">{shortcutError}</p>{/if}
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
            <span class="hint">{t('settings.startWithSystemHint')}</span>
          </div>
          <div class="control end">
            <button
              type="button"
              class="switch"
              role="switch"
              aria-checked={settings.startWithSystem}
              aria-labelledby="label-autostart"
              onclick={() => change({ startWithSystem: !settings.startWithSystem })}
            >
              <i></i>
            </button>
          </div>
        </div>
      </div>
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

  .segments {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 2px;
    padding: 3px;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.055);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08);
  }

  .segments button {
    height: 30px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: var(--text-dim);
    font: inherit;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out;
  }

  .segments button:hover:not(.on) {
    color: var(--text);
  }

  .segments button.on {
    background: var(--raised);
    color: var(--text);
    box-shadow:
      0 0 0 1px rgba(255, 255, 255, 0.1),
      0 2px 6px rgba(0, 0, 0, 0.35);
  }

  .segments button:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 1px;
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

  @media (prefers-reduced-motion: reduce) {
    .bar.indeterminate i {
      animation: none;
      width: 100%;
      opacity: 0.5;
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
