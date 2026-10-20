<script lang="ts" module>
  // One line style for every icon: 24 px grid, 1.5 px stroke at any size, round caps and joins.
  const ICONS = {
    home: '<path d="M4.5 10.2 12 4.2l7.5 6V19a1 1 0 0 1-1 1h-4v-5.5h-5V20h-4a1 1 0 0 1-1-1z"/>',
    heart:
      '<path d="M12 19.3s-7.3-4.4-7.3-9.7a4 4 0 0 1 7.3-2.3 4 4 0 0 1 7.3 2.3c0 5.3-7.3 9.7-7.3 9.7z"/>',
    playlists:
      '<path d="M4 6.5h12M4 11.5h12M4 16.5h7"/><path d="M18 18.5V12l2.5 1"/><circle cx="16.2" cy="18.5" r="1.8"/>',
    album: '<circle cx="12" cy="12" r="8"/><circle cx="12" cy="12" r="2.2"/>',
    artist: '<circle cx="12" cy="8.5" r="3.5"/><path d="M5 20a7 7 0 0 1 14 0"/>',
    songs: '<path d="M9 17.5V6l10-2v11.5"/><circle cx="6.5" cy="17.5" r="2.5"/><circle cx="16.5" cy="15.5" r="2.5"/>',
    settings:
      '<path d="M4 7.5h8.5M16.5 7.5H20M4 16.5h3.5M11.5 16.5H20"/><circle cx="14.5" cy="7.5" r="2"/><circle cx="9.5" cy="16.5" r="2"/>',
    prev: '<path d="M6.5 5.5v13"/><path d="M18.5 6.6v10.8a.9.9 0 0 1-1.4.7L9.6 12.7a.9.9 0 0 1 0-1.4l7.5-5.4a.9.9 0 0 1 1.4.7z"/>',
    next: '<path d="M17.5 5.5v13"/><path d="M5.5 6.6v10.8a.9.9 0 0 0 1.4.7l7.5-5.4a.9.9 0 0 0 0-1.4L6.9 5.9a.9.9 0 0 0-1.4.7z"/>',
    volume:
      '<path d="M4.5 9.5H7.5L12 5.5v13l-4.5-4H4.5z"/><path d="M15.5 9.2a4 4 0 0 1 0 5.6M18 6.8a7.4 7.4 0 0 1 0 10.4"/>',
    volumeLow: '<path d="M4.5 9.5H7.5L12 5.5v13l-4.5-4H4.5z"/><path d="M15.5 9.2a4 4 0 0 1 0 5.6"/>',
    mute: '<path d="M4.5 9.5H7.5L12 5.5v13l-4.5-4H4.5z"/><path d="m16 9.5 5 5M21 9.5l-5 5"/>',
    minimize: '<path d="M6 12h12"/>',
    maximize: '<rect x="6" y="6" width="12" height="12" rx="2"/>',
    restore:
      '<rect x="5.5" y="8.5" width="10" height="10" rx="1.8"/><path d="M8.5 5.5h8.2a1.8 1.8 0 0 1 1.8 1.8v8.2"/>',
    close: '<path d="m6.5 6.5 11 11M17.5 6.5l-11 11"/>',
    play: '<path d="M7.5 5.6v12.8a.9.9 0 0 0 1.4.8l10-6.4a.9.9 0 0 0 0-1.6l-10-6.4a.9.9 0 0 0-1.4.8z"/>',
    pause: '<rect x="6.5" y="5.5" width="3.5" height="13" rx="1"/><rect x="14" y="5.5" width="3.5" height="13" rx="1"/>',
    shuffle:
      '<path d="M4 7.5h3.2c4.6 0 5 9 9.6 9H20M4 16.5h3.2c1.6 0 2.6-1.1 3.4-2.6M13.4 10.1c.8-1.5 1.8-2.6 3.4-2.6H20"/><path d="m17.5 5 2.5 2.5-2.5 2.5M17.5 14l2.5 2.5-2.5 2.5"/>',
    search: '<circle cx="10.5" cy="10.5" r="6"/><path d="m15 15 5 5"/>',
    folderAdd:
      '<path d="M3.5 7.5v10a1.5 1.5 0 0 0 1.5 1.5h14a1.5 1.5 0 0 0 1.5-1.5v-8A1.5 1.5 0 0 0 19 8h-7l-2-2.5H5A1.5 1.5 0 0 0 3.5 7z"/><path d="M12 11v5M9.5 13.5h5"/>',
    back: '<path d="M14.5 6 8.5 12l6 6"/>',
    folder: '<path d="M4 7.5A1.5 1.5 0 0 1 5.5 6h4l2 2h7A1.5 1.5 0 0 1 20 9.5v8a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 17.5z"/>',
    chevronDown: '<path d="m7 10 5 5 5-5"/>',
    chevronRight: '<path d="m10 7 5 5-5 5"/>',
    plus: '<path d="M12 5.5v13M5.5 12h13"/>',
    edit: '<path d="M14.5 6.5 17.5 9.5"/><path d="M5.5 18.5 6.3 15 15.8 5.5a1.6 1.6 0 0 1 2.2 0l.5.5a1.6 1.6 0 0 1 0 2.2L9 17.7z"/>',
    trash:
      '<path d="M5 7.5h14M10 7.5V5.8a.8.8 0 0 1 .8-.8h2.4a.8.8 0 0 1 .8.8v1.7"/><path d="M6.8 7.5l.8 11a1.5 1.5 0 0 0 1.5 1.4h5.8a1.5 1.5 0 0 0 1.5-1.4l.8-11"/>',
    grip: '<circle cx="9.5" cy="7" r=".9"/><circle cx="14.5" cy="7" r=".9"/><circle cx="9.5" cy="12" r=".9"/><circle cx="14.5" cy="12" r=".9"/><circle cx="9.5" cy="17" r=".9"/><circle cx="14.5" cy="17" r=".9"/>',
    remove: '<path d="M6 12h12"/><circle cx="12" cy="12" r="8.5"/>',
    check: '<path d="m5.5 12.5 4 4 9-9"/>',
    refresh: '<path d="M19 12a7 7 0 1 1-2.05-4.95"/><path d="M19 4.5v4h-4"/>',
    external: '<path d="M13.5 5.5H18.5V10.5"/><path d="M18.5 5.5 11 13"/><path d="M16.5 14v3.5a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1v-9a1 1 0 0 1 1-1H10"/>',
    drop: '<path d="M12 4v11M7.5 10.5 12 15l4.5-4.5"/><path d="M5 15.5V18a1.5 1.5 0 0 0 1.5 1.5h11A1.5 1.5 0 0 0 19 18v-2.5"/>',
  } as const;

  export type IconName = keyof typeof ICONS;
</script>

<script lang="ts">
  let { name, size = 18, filled = false }: { name: IconName; size?: number; filled?: boolean } = $props();
</script>

<svg
  class="icon"
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill={filled ? 'currentColor' : 'none'}
  stroke="currentColor"
  stroke-width="1.5"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {@html ICONS[name]}
</svg>

<style>
  .icon {
    display: block;
    flex: none;
  }
  .icon :global(*) {
    vector-effect: non-scaling-stroke;
  }
</style>
