<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import { t } from '../i18n';
  import { playerSeek } from '../ipc';
  import { activeLine, FOLLOW_PAUSE_MS, interpolate } from '../lyrics';
  import { lyricsState, retryLyrics, syncLyrics } from '../stores/lyrics.svelte';
  import { go } from '../stores/nav.svelte';
  import { player } from '../stores/player.svelte';
  import { settings } from '../stores/settings.svelte';
  import { moveTo, ytm } from '../stores/ytm.svelte';

  /** The lyrics of the song that plays: synced lines follow the song and a click on one
   *  jumps there, plain lyrics just scroll. `compact` is the side panel's smaller type. */
  let { compact = false }: { compact?: boolean } = $props();

  $effect(() => syncLyrics());

  const view = $derived(lyricsState.view);
  const lyrics = $derived(view.kind === 'found' && !view.lyrics.instrumental ? view.lyrics : null);
  const synced = $derived(!!lyrics?.synced);

  // The drawn position: the engine reports every 250 ms, YouTube Music every 100 ms, and
  // between two reports the clock carries it on, so lines change when they are sung.
  const yt = $derived(player.active === 'ytm');
  const playing = $derived(yt ? ytm.state.status === 'playing' : player.status === 'playing');
  const reported = $derived(yt ? ytm.positionMs : player.positionMs);
  const duration = $derived(yt ? ytm.state.durationMs : player.durationMs);
  let base = { positionMs: 0, at: 0 };
  let position = $state(0);

  /** After a seek, a report far from the target is the engine catching up; it is ignored
   *  until one near the target arrives or the hold runs out. */
  let hold: { ms: number; until: number } | null = null;
  const HOLD_MS = 600;
  const NEAR_MS = 1500;

  $effect(() => {
    const ms = reported;
    untrack(() => {
      const now = performance.now();
      if (hold) {
        if (now < hold.until && Math.abs(ms - hold.ms) > NEAR_MS) return;
        hold = null;
      }
      base = { positionMs: ms, at: now };
      position = ms;
    });
  });

  // No frames while paused or when there is nothing to follow.
  $effect(() => {
    if (!playing || !synced) return;
    const limit = duration;
    // The last report may be from before a pause: count from the drawn position, now.
    untrack(() => (base = { positionMs: position, at: performance.now() }));
    let frame = requestAnimationFrame(function step() {
      position = interpolate(base, performance.now(), true, limit);
      frame = requestAnimationFrame(step);
    });
    return () => cancelAnimationFrame(frame);
  });

  const active = $derived(lyrics?.synced ? activeLine(lyrics.lines, position) : -1);

  let scroller = $state<HTMLDivElement>();
  let following = $state(true);
  /** The line with focus: the one Tab stop of the lines, which the arrow keys move. */
  let focused = $state(-1);
  const firstLine = $derived(lyrics ? lyrics.lines.findIndex((line) => line.text !== '') : -1);
  const tabStop = $derived(
    focused >= 0 ? focused : active >= 0 && lyrics?.lines[active]?.text ? active : firstLine,
  );
  let resumeTimer: ReturnType<typeof setTimeout> | undefined;

  function reducedMotion(): boolean {
    return typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;
  }

  /** Puts a line near the vertical center. */
  function reveal(index: number) {
    const el = scroller?.querySelector<HTMLElement>(`[data-line="${index}"]`);
    if (!scroller || !el) return;
    const top = Math.max(0, Math.round(el.offsetTop - (scroller.clientHeight - el.offsetHeight) / 2));
    if (!reducedMotion() && typeof scroller.scrollTo === 'function') {
      scroller.scrollTo({ top, behavior: 'smooth' });
    } else {
      scroller.scrollTop = top;
    }
  }

  // Follows the song; resuming (following turns true again) brings the current line back.
  $effect(() => {
    const index = active;
    if (!following || index < 0 || !scroller) return;
    untrack(() => reveal(index));
  });

  // A new song starts at the top and follows again.
  $effect(() => {
    void lyricsState.key;
    untrack(() => {
      clearTimeout(resumeTimer);
      following = true;
      focused = -1;
      if (scroller) scroller.scrollTop = 0;
    });
  });

  function pauseFollowing() {
    if (!synced) return;
    following = false;
    clearTimeout(resumeTimer);
    resumeTimer = setTimeout(() => (following = true), FOLLOW_PAUSE_MS);
  }

  function resumeFollowing() {
    clearTimeout(resumeTimer);
    following = true;
  }

  onDestroy(() => clearTimeout(resumeTimer));

  // Wheel, touch, a drag with the button down (past a few px, so a click's jitter does not
  // count) and the scrolling keys are the user's own scrolling; the view's own does not.
  const DRAG_PX = 4;
  let downY: number | null = null;

  function onpointerdown(event: PointerEvent) {
    downY = event.clientY;
  }

  function onpointermove(event: PointerEvent) {
    if (event.buttons & 1 && downY !== null && Math.abs(event.clientY - downY) > DRAG_PX) pauseFollowing();
  }

  const SCROLL_KEYS = new Set(['PageUp', 'PageDown', 'Home', 'End', 'ArrowUp', 'ArrowDown']);

  function onkeydown(event: KeyboardEvent) {
    if (!SCROLL_KEYS.has(event.key)) return;
    pauseFollowing();
    if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown') return;
    event.preventDefault();
    const buttons = [...(scroller?.querySelectorAll<HTMLElement>('button.line') ?? [])];
    const at = buttons.indexOf(document.activeElement as HTMLElement);
    const next = buttons[at + (event.key === 'ArrowDown' ? 1 : -1)];
    if (at < 0 || !next) return;
    focused = Number(next.dataset.line);
    next.focus();
  }

  function onfocusin(event: FocusEvent) {
    const line = (event.target as HTMLElement).closest<HTMLElement>('button.line');
    if (line) focused = Number(line.dataset.line);
  }

  function seek(startMs: number | null) {
    if (startMs === null) return;
    const now = performance.now();
    base = { positionMs: startMs, at: now };
    position = startMs;
    hold = { ms: startMs, until: now + HOLD_MS };
    // YouTube Music's drawn position runs on in the store until told otherwise.
    if (yt) moveTo(startMs);
    playerSeek(startMs).catch((err) => console.error('seeking failed', err));
  }

  const sourceKey = $derived(lyrics ? (`lyrics.source.${lyrics.source}` as const) : null);
</script>

<div class="lyrics" class:compact>
  {#if view.kind === 'found' && view.lyrics.instrumental}
    <p class="note big">{t('lyrics.instrumental')}</p>
  {:else if lyrics}
    {#if !lyrics.synced}<p class="unsynced">{t('lyrics.notSynced')}</p>{/if}
    <!-- The scroller only listens for the user's own scrolling; its lines are the buttons. -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="scroll"
      bind:this={scroller}
      onwheel={pauseFollowing}
      ontouchmove={pauseFollowing}
      {onpointerdown}
      {onpointermove}
      {onkeydown}
      {onfocusin}
    >
      <ol class="lines" class:plain={!lyrics.synced}>
        {#each lyrics.lines as line, i (i)}
          {#if line.text === ''}
            <li class="gap" aria-hidden="true"></li>
          {:else if lyrics.synced}
            <li>
              <button
                type="button"
                class="line"
                class:past={i < active}
                class:now={i === active}
                data-line={i}
                tabindex={i === tabStop ? 0 : -1}
                aria-current={i === active ? 'true' : undefined}
                onclick={() => seek(line.startMs)}>{line.text}</button
              >
            </li>
          {:else}
            <li class="line text" data-line={i}>{line.text}</li>
          {/if}
        {/each}
      </ol>
    </div>
    {#if synced && !following}
      <button type="button" class="back" onclick={resumeFollowing}>{t('lyrics.backToCurrent')}</button>
    {/if}
    {#if sourceKey}<p class="source">{t(sourceKey)}</p>{/if}
  {:else if view.kind === 'loading'}
    <p class="note" role="status">{t('lyrics.loading')}</p>
  {:else if view.kind === 'none'}
    <div class="note-block">
      <p class="note big">{t('lyrics.none')}</p>
      {#if !settings.lyricsLookup}
        <p class="note">{t('lyrics.onlineOff')}</p>
        <button type="button" class="action" onclick={() => go('settings')}>{t('lyrics.openSettings')}</button>
      {/if}
    </div>
  {:else if view.kind === 'error'}
    <div class="note-block">
      <p class="note big">{t('lyrics.error')}</p>
      <button type="button" class="action" onclick={retryLyrics}>{t('lyrics.retry')}</button>
    </div>
  {:else if view.kind === 'radio'}
    <p class="note big">{t('lyrics.radio')}</p>
  {:else}
    <p class="note big">{t('lyrics.idle')}</p>
  {/if}
</div>

<style>
  .lyrics {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    --size: clamp(20px, 2vw, 28px);
  }

  .lyrics.compact {
    --size: 19px;
  }

  .scroll {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    scrollbar-width: none;
    /* Lines fade out at both edges. */
    mask-image: linear-gradient(transparent, #000 12%, #000 88%, transparent);
  }

  .scroll::-webkit-scrollbar {
    display: none;
  }

  .lines {
    margin: 0;
    padding: 36% 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 0.5em;
    font-family: var(--font-display);
    font-size: var(--size);
    font-weight: 600;
    letter-spacing: -0.015em;
    line-height: 1.25;
  }

  /* Plain lyrics read from the top; there is nothing to center. */
  .lines.plain {
    padding: 4px 0 24px;
    font-weight: 500;
    font-size: calc(var(--size) * 0.8);
    gap: 0.35em;
  }

  .gap {
    height: 0.8em;
  }

  .line {
    display: block;
    width: 100%;
    margin: 0;
    padding: 2px 4px;
    border: 0;
    border-radius: var(--radius-s);
    background: none;
    color: var(--text);
    font: inherit;
    letter-spacing: inherit;
    line-height: inherit;
    text-align: left;
    text-wrap: pretty;
    overflow-wrap: anywhere;
    opacity: 0.55;
    cursor: pointer;
    transition:
      opacity var(--dur) ease-out,
      text-shadow var(--dur) ease-out;
  }

  .line.text {
    opacity: 0.85;
    cursor: text;
  }

  .line.past {
    opacity: 0.3;
  }

  .line:hover {
    opacity: 0.85;
  }

  .line.now {
    opacity: 1;
    color: #fff;
    /* A soft spectral glow; static, no animation. */
    text-shadow:
      0 0 18px color-mix(in srgb, var(--spec-4) 40%, transparent),
      0 0 42px color-mix(in srgb, var(--spec-1) 22%, transparent);
  }

  .unsynced {
    flex: none;
    margin: 0 0 6px;
    font-size: 12px;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--text-dim);
  }

  .source {
    flex: none;
    margin: 8px 0 0;
    font-size: 12px;
    color: var(--text-dim);
  }

  .back {
    position: absolute;
    left: 50%;
    bottom: 36px;
    transform: translateX(-50%);
    padding: 7px 14px;
    border: 0;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.14);
    color: var(--text);
    font: inherit;
    font-size: 13px;
    cursor: pointer;
    backdrop-filter: blur(12px);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.12);
    transition: background-color var(--dur) ease-out;
  }

  .back:hover {
    background: rgba(255, 255, 255, 0.22);
  }

  .note-block {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
    padding: 16px 0;
  }

  .note {
    margin: 0;
    font-size: 14px;
    color: var(--text-dim);
  }

  .note.big {
    padding: 16px 0 0;
    font-family: var(--font-display);
    font-size: 18px;
    font-weight: 600;
    color: var(--text);
  }

  .note-block .note.big {
    padding: 0;
  }

  .action {
    margin-top: 4px;
    padding: 7px 14px;
    border: 0;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.1);
    color: var(--text);
    font: inherit;
    font-size: 13px;
    cursor: pointer;
    transition: background-color var(--dur) ease-out;
  }

  .action:hover {
    background: rgba(255, 255, 255, 0.18);
  }
</style>
