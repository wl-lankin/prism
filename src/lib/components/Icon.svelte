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
  } as const;

  export type IconName = keyof typeof ICONS;
</script>

<script lang="ts">
  let { name, size = 18 }: { name: IconName; size?: number } = $props();
</script>

<svg
  class="icon"
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
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
