<script lang="ts" module>
  export type RadioTab = 'near' | 'search' | 'saved';

  /** The tab a nav param names; anything else is "near you". */
  export function tabOf(param: string | undefined): RadioTab {
    return param === 'search' || param === 'saved' ? param : 'near';
  }
</script>

<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { t } from '../i18n';
  import { saveScroll, savedScroll } from '../scroll';
  import { go, nav } from '../stores/nav.svelte';
  import {
    countryName,
    facets,
    loadCountries,
    loadNear,
    loadRegions,
    loadSaved,
    loadTags,
    radio,
    runSearch,
    searchMore,
  } from '../stores/radio.svelte';
  import Icon from './Icon.svelte';
  import Segmented from './Segmented.svelte';
  import Select from './Select.svelte';
  import StationList from './StationList.svelte';
  import ViewHead from './ViewHead.svelte';

  const tab = $derived(tabOf(nav.param));
  const KEY = $derived(`radio:${tab}`);

  const tabs = $derived([
    { value: 'near' as RadioTab, label: t('radio.tab.near') },
    { value: 'search' as RadioTab, label: t('radio.tab.search') },
    { value: 'saved' as RadioTab, label: t('radio.tab.saved') },
  ]);

  /** "Near Testhausen, Germany" for a detected place, "Germany, Bavaria" or "Germany" otherwise. */
  const placeLine = $derived.by(() => {
    const place = radio.place;
    if (!place) return '';
    const country = countryName(place.countryCode);
    if (place.origin === 'detected' && place.city) return t('radio.near.place', { city: place.city, country });
    return place.region ? t('radio.near.country', { country, region: place.region }) : country;
  });

  const nearEmpty = $derived(!radio.near.loading && radio.near.stations.length === 0);
  const nearHint = $derived(radio.near.error !== null && (radio.near.cached || radio.near.stale));

  // The search filters start from the last search, which the store keeps across tabs.
  const last = untrack(() => radio.search.query);
  let name = $state(last.name ?? '');
  let country = $state(last.countryCode ?? '');
  let region = $state(last.region ?? '');
  let tag = $state(last.tag ?? '');
  /** A search is on its way (typing, debounce or request), so "no stations" is not said yet. */
  let waiting = $state(false);
  let sawLoading = false;

  /** "Anywhere" first, then the options; a repeated value is dropped (the list keys by value). */
  function withAnywhere(options: { value: string; label: string }[]) {
    const seen = new Set(['']);
    return [
      { value: '', label: t('radio.search.anywhere') },
      ...options.filter((option) => !seen.has(option.value) && seen.add(option.value)),
    ];
  }

  const countryOptions = $derived(
    withAnywhere(
      [...(facets.countries ?? [])]
        .sort((a, b) => a.name.localeCompare(b.name))
        .map((c) => ({ value: c.code, label: c.name })),
    ),
  );
  const regionOptions = $derived(withAnywhere(facets.regions.map((r) => ({ value: r.name, label: r.name }))));
  const tagOptions = $derived(withAnywhere((facets.tags ?? []).map((g) => ({ value: g.name, label: g.name }))));

  const filtered = $derived(Boolean(name.trim() || country || region || tag));

  function search() {
    waiting = filtered;
    runSearch({
      name: name.trim() || undefined,
      countryCode: country || undefined,
      region: region || undefined,
      tag: tag || undefined,
    });
  }

  function pickCountry(code: string) {
    country = code;
    region = '';
    void loadRegions(code);
    search();
  }

  $effect(() => {
    if (radio.search.loading) sawLoading = true;
    else if (sawLoading) {
      sawLoading = false;
      waiting = false;
    }
  });

  onMount(() => {
    void loadSaved();
    if (tab === 'near') void loadNear();
    if (tab === 'search') {
      void loadCountries();
      void loadTags();
      if (country && facets.regionsFor !== country) void loadRegions(country);
    }
  });

  let scroller = $state<HTMLDivElement>();
  let viewport = $state(0);

  $effect(() => {
    if (!scroller || viewport === 0) return;
    const top = savedScroll(KEY);
    if (top) scroller.scrollTop = top;
  });

  function onscroll() {
    if (scroller) saveScroll(KEY, scroller.scrollTop);
  }
</script>

<div class="scroll" bind:this={scroller} bind:clientHeight={viewport} {onscroll}>
  <ViewHead title={t('radio.title')} meta={tab === 'near' ? placeLine : undefined} {actions} />

  {#if tab === 'near'}
    {#if nearHint}
      <p class="hint" role="note">{t('radio.near.cached')}</p>
    {/if}
    {#if radio.near.stations.length > 0}
      <StationList stations={radio.near.stations} showDistance />
    {:else if radio.near.loading}
      <p class="note" role="status">{t('radio.near.loading')}</p>
    {:else if nearEmpty}
      <div class="empty">
        <span class="emblem" aria-hidden="true"><Icon name="radio" size={26} /></span>
        <p class="lead">{radio.near.error ? t('radio.error.unreachable') : t('radio.near.empty')}</p>
        <button type="button" class="btn ghost" onclick={() => go('settings')}>{t('sourceEmpty.openSettings')}</button>
      </div>
    {/if}
  {:else if tab === 'search'}
    <div class="filters">
      <input
        type="search"
        class="field"
        placeholder={t('radio.search.placeholder')}
        aria-label={t('radio.search.placeholder')}
        autocomplete="off"
        spellcheck="false"
        bind:value={name}
        oninput={search}
      />
      <div class="pick">
        <span class="caption">{t('radio.search.country')}</span>
        <Select value={country} options={countryOptions} label={t('radio.search.country')} onchange={pickCountry} />
      </div>
      <div class="pick">
        <span class="caption">{t('radio.search.region')}</span>
        <Select
          value={region}
          options={regionOptions}
          label={t('radio.search.region')}
          disabled={!country}
          onchange={(value) => {
            region = value;
            search();
          }}
        />
      </div>
      <div class="pick">
        <span class="caption">{t('radio.search.genre')}</span>
        <Select
          value={tag}
          options={tagOptions}
          label={t('radio.search.genre')}
          onchange={(value) => {
            tag = value;
            search();
          }}
        />
      </div>
    </div>

    {#if radio.search.error}
      <p class="note error" role="alert">{t('radio.error.unreachable')}</p>
    {:else if filtered && !waiting && !radio.search.loading && radio.search.stations.length === 0}
      <p class="note">{t('radio.search.empty')}</p>
    {/if}
    <StationList stations={radio.search.stations} />
    {#if radio.search.more}
      <div class="more">
        <button type="button" class="btn ghost" disabled={radio.search.loading} onclick={() => void searchMore()}>
          {t('radio.search.more')}
        </button>
      </div>
    {/if}
  {:else}
    {#if radio.saved.favorites.length === 0 && radio.saved.recent.length === 0}
      <p class="note">{t('radio.saved.empty')}</p>
    {/if}
    {#if radio.saved.favorites.length > 0}
      <h2>{t('radio.saved.favorites')}</h2>
      <StationList stations={radio.saved.favorites} virtual={false} />
    {/if}
    {#if radio.saved.recent.length > 0}
      <h2>{t('radio.saved.recent')}</h2>
      <StationList stations={radio.saved.recent} virtual={false} />
    {/if}
  {/if}
</div>

{#snippet actions()}
  {#if tab === 'near'}
    <button
      type="button"
      class="refresh"
      disabled={radio.near.loading}
      onclick={() => void loadNear(true)}
    >
      <Icon name="refresh" size={15} />
      <span>{t('radio.refresh')}</span>
    </button>
  {/if}
  <span id="radio-tab-label" class="sr-only">{t('radio.title')}</span>
  <Segmented
    options={tabs}
    value={tab}
    labelledby="radio-tab-label"
    attr="data-tab"
    onpick={(value) => go('radio', value)}
  />
{/snippet}

<style>
  .scroll {
    scrollbar-color: auto;
    position: relative;
    height: 100%;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 4px 32px 40px;
    box-sizing: border-box;
  }

  .refresh {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 36px;
    padding: 0 12px;
    border: 0;
    border-radius: 10px;
    background: transparent;
    color: var(--text-dim);
    font: inherit;
    font-size: 13px;
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out;
  }

  .refresh:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.06);
    color: var(--text);
  }

  .refresh:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .refresh:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 1px;
  }

  .hint,
  .note {
    margin: 0 0 14px;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-dim);
  }

  .hint {
    padding: 9px 12px;
    border-radius: 9px;
    background: rgba(255, 255, 255, 0.04);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.06);
  }

  .note.error {
    color: var(--spec-1);
  }

  .filters {
    display: grid;
    grid-template-columns: minmax(0, 1.6fr) repeat(3, minmax(0, 1fr));
    gap: 12px;
    align-items: end;
    margin-bottom: 18px;
  }

  .pick {
    display: grid;
    gap: 6px;
    min-width: 0;
  }

  .caption {
    font-size: 11.5px;
    color: rgba(243, 243, 245, 0.4);
  }

  .field {
    height: 36px;
    min-width: 0;
    padding: 0 12px;
    box-sizing: border-box;
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

  .field::placeholder {
    color: rgba(243, 243, 245, 0.4);
  }

  .field:focus-visible {
    background: rgba(255, 255, 255, 0.075);
    box-shadow: inset 0 0 0 1.5px rgba(77, 166, 255, 0.7);
  }

  .more {
    display: flex;
    justify-content: center;
    padding-top: 18px;
  }

  h2 {
    margin: 22px 12px 8px;
    font-size: 11.5px;
    font-weight: 500;
    letter-spacing: 0.01em;
    color: rgba(243, 243, 245, 0.4);
  }

  h2:first-of-type {
    margin-top: 4px;
  }

  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    max-width: 420px;
    margin: 72px auto 0;
    text-align: center;
  }

  .emblem {
    width: 64px;
    height: 64px;
    display: grid;
    place-items: center;
    margin-bottom: 20px;
    border-radius: 50%;
    color: var(--src-radio);
    background: radial-gradient(circle, color-mix(in srgb, var(--src-radio) 22%, transparent) 0%, transparent 70%);
  }

  .lead {
    margin: 0 0 22px;
    font-size: 14px;
    line-height: 1.6;
    color: var(--text-dim);
    text-wrap: pretty;
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
</style>
