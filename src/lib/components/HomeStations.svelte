<script lang="ts">
  import { t } from '../i18n';
  import { favorites } from '../stores/favorites.svelte';
  import { loadSaved, radio } from '../stores/radio.svelte';
  import StationArt from './StationArt.svelte';
  import { playStationNotified } from './stationUi';

  /** Home's "Your stations" row: up to 12 favorite stations, nothing while there are none. */
  const MAX = 12;

  $effect(() => {
    void favorites.version;
    void loadSaved();
  });

  // A heart taken off elsewhere removes the tile before the saved list reloads.
  const shown = $derived(radio.saved.favorites.filter((x) => favorites.stations.has(x.id)).slice(0, MAX));
</script>

{#if shown.length}
  <section class="stations" aria-labelledby="home-stations">
    <h2 id="home-stations">{t('home.stations')}</h2>
    <ul>
      {#each shown as station (station.id)}
        <li>
          <button type="button" class="tile" onclick={() => playStationNotified(station)}>
            <StationArt {station} size={96} radius="12px" />
            <span class="name">{station.name}</span>
          </button>
        </li>
      {/each}
    </ul>
  </section>
{/if}

<style>
  .stations {
    margin: -6px 0 28px;
  }

  h2 {
    margin: 0 0 12px;
    font-family: var(--font-display);
    font-size: 17px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    gap: 16px;
    overflow-x: auto;
    padding-bottom: 6px;
  }

  .tile {
    width: 96px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
    border-radius: 12px;
  }

  .tile:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 3px;
  }

  .tile :global(.art) {
    transition: filter var(--dur) ease-out;
  }

  .tile:hover :global(.art) {
    filter: brightness(1.12);
  }

  .name {
    max-width: 100%;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-size: 12.5px;
    font-weight: 500;
  }
</style>
