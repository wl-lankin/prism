<script lang="ts" module>
  export const HOMEPAGE = 'https://wolfgang-linz.de';
  export const REPOSITORY = 'https://github.com/wl-lankin/prism';

  /** The main open-source parts of Prism. A static list for now; release builds generate
   *  the full one. */
  export const LICENSES: { name: string; license: string; external?: boolean }[] = [
    { name: 'Tauri', license: 'MIT / Apache-2.0' },
    { name: 'Svelte', license: 'MIT' },
    { name: 'symphonia', license: 'MPL-2.0' },
    { name: 'cpal', license: 'Apache-2.0' },
    { name: 'rusqlite', license: 'MIT' },
    { name: 'lofty', license: 'MIT / Apache-2.0' },
    { name: 'FFmpeg', license: 'LGPL-2.1+', external: true },
    { name: 'Inter', license: 'OFL-1.1' },
    { name: 'Space Grotesk', license: 'OFL-1.1' },
  ];
</script>

<script lang="ts">
  import { getVersion } from '@tauri-apps/api/app';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { onMount, tick } from 'svelte';
  import logoUrl from '../../../assets/logo.svg';
  import { t } from '../i18n';
  import Icon from './Icon.svelte';

  /** The About dialog: modal, focus stays inside, Esc or a click on the backdrop closes it
   *  and focus returns to where it was. */
  let { onclose }: { onclose: () => void } = $props();

  let dialog: HTMLDivElement | undefined = $state();
  let version = $state('');

  onMount(() => {
    const before = document.activeElement as HTMLElement | null;
    getVersion()
      .then((v) => (version = v))
      .catch(() => {});
    void tick().then(() => dialog?.querySelector<HTMLElement>('.close')?.focus());
    return () => before?.focus?.();
  });

  function focusables(): HTMLElement[] {
    return [...(dialog?.querySelectorAll<HTMLElement>('a[href], button:not([disabled]), [tabindex="0"]') ?? [])];
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      onclose();
      return;
    }
    if (event.key !== 'Tab') return;
    const items = focusables();
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    const inside = dialog?.contains(document.activeElement);
    if (event.shiftKey && (document.activeElement === first || !inside)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (document.activeElement === last || !inside)) {
      event.preventDefault();
      first.focus();
    }
  }

  function link(event: MouseEvent) {
    event.preventDefault();
    const href = (event.currentTarget as HTMLAnchorElement).href;
    openUrl(href).catch((err) => console.error('opening the link failed', err));
  }

  // A press that starts on the backdrop closes; one that started inside (a text selection
  // dragged out) does not.
  let pressedBackdrop = false;
</script>

<svelte:window {onkeydown} />

<div
  class="backdrop"
  role="presentation"
  onpointerdown={(event) => (pressedBackdrop = event.target === event.currentTarget)}
  onclick={(event) => {
    if (pressedBackdrop && event.target === event.currentTarget) onclose();
    pressedBackdrop = false;
  }}
>
  <div bind:this={dialog} class="dialog" role="dialog" aria-modal="true" aria-labelledby="about-title">
    <button type="button" class="close" aria-label={t('about.close')} onclick={onclose}>
      <Icon name="close" size={18} />
    </button>

    <div class="mark" aria-hidden="true">
      <img src={logoUrl} alt="" width="76" height="76" draggable="false" />
    </div>
    <h2 id="about-title">Prism</h2>
    <p class="version">{version ? t('about.version', { version }) : ''}</p>
    <p class="tagline">{t('about.tagline')}</p>

    <p class="credit">
      {t('about.madeBy')}
      <a href={HOMEPAGE} onclick={link}>Wolfgang Linz</a>
    </p>
    <a class="github" href={REPOSITORY} onclick={link}>
      {t('about.github')}
      <Icon name="external" size={14} />
    </a>

    <section class="licenses" aria-labelledby="about-licenses">
      <h3 id="about-licenses">{t('about.licenses')}</h3>
      <p>{t('about.licensesLead')}</p>
      <dl>
        {#each LICENSES as item (item.name)}
          <div>
            <dt>
              {item.name}
              {#if item.external}<span class="external">{t('about.external')}</span>{/if}
            </dt>
            <dd>{item.license}</dd>
          </div>
        {/each}
      </dl>
    </section>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 20;
    display: grid;
    place-items: center;
    padding: 24px;
    background: rgba(8, 8, 10, 0.66);
    backdrop-filter: blur(8px);
    animation: fade var(--dur) ease-out;
  }

  .dialog {
    position: relative;
    width: min(420px, 100%);
    max-height: calc(100vh - 48px);
    overflow-y: auto;
    box-sizing: border-box;
    padding: 36px 32px 26px;
    border-radius: 20px;
    background: var(--surface);
    box-shadow:
      inset 0 0 0 1px rgba(255, 255, 255, 0.08),
      0 30px 80px -20px rgba(0, 0, 0, 0.85),
      0 10px 30px rgba(0, 0, 0, 0.45);
    text-align: center;
    user-select: text;
    -webkit-user-select: text;
    animation: rise var(--dur-l) cubic-bezier(0.2, 0.8, 0.2, 1);
    scrollbar-color: auto;
  }

  .close {
    position: absolute;
    top: 12px;
    right: 12px;
    width: 32px;
    height: 32px;
    display: grid;
    place-items: center;
    border: 0;
    border-radius: 9px;
    background: transparent;
    color: var(--text-dim);
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out;
  }

  .close:hover {
    background: rgba(255, 255, 255, 0.07);
    color: var(--text);
  }

  .close:focus-visible,
  a:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 2px;
  }

  /* The logo on a pool of the spectrum, as on the empty library. */
  .mark {
    position: relative;
    width: 76px;
    margin: 0 auto 20px;
  }

  .mark::before {
    content: '';
    position: absolute;
    inset: -20px;
    border-radius: 50%;
    background: conic-gradient(var(--spec-1), var(--spec-2), var(--spec-3), var(--spec-4), var(--spec-1));
    filter: blur(28px);
    opacity: 0.34;
  }

  .mark img {
    position: relative;
    display: block;
    border-radius: 18px;
    box-shadow:
      0 0 0 1px rgba(255, 255, 255, 0.1),
      0 14px 32px rgba(0, 0, 0, 0.55);
  }

  h2 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 30px;
    font-weight: 600;
    letter-spacing: -0.02em;
    line-height: 1.1;
  }

  .version {
    min-height: 18px;
    margin: 6px 0 0;
    font-size: 12.5px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }

  .tagline {
    margin: 16px 0 0;
    font-family: var(--font-display);
    font-size: 16px;
    font-weight: 500;
    letter-spacing: -0.005em;
  }

  .credit {
    margin: 18px 0 0;
    font-size: 13.5px;
    color: var(--text-dim);
  }

  a {
    color: var(--text);
    text-decoration: underline;
    text-decoration-color: rgba(243, 243, 245, 0.3);
    text-underline-offset: 3px;
    border-radius: 3px;
    transition: text-decoration-color var(--dur) ease-out;
  }

  a:hover {
    text-decoration-color: var(--spec-4);
  }

  .github {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    margin-top: 8px;
    font-size: 13.5px;
  }

  .licenses {
    margin-top: 26px;
    padding-top: 20px;
    border-top: 1px solid var(--line);
    text-align: left;
  }

  h3 {
    margin: 0;
    font-size: 13.5px;
    font-weight: 600;
  }

  .licenses p {
    margin: 4px 0 12px;
    font-size: 12.5px;
    line-height: 1.5;
    color: var(--text-dim);
  }

  dl {
    margin: 0;
    display: grid;
    gap: 1px;
  }

  dl div {
    display: flex;
    align-items: baseline;
    gap: 12px;
    padding: 6px 0;
    font-size: 12.5px;
  }

  dl div + div {
    border-top: 1px solid rgba(255, 255, 255, 0.04);
  }

  dt {
    flex: 1;
    min-width: 0;
  }

  .external {
    margin-left: 6px;
    color: var(--text-dim);
  }

  dd {
    margin: 0;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  @keyframes fade {
    from {
      opacity: 0;
    }
  }

  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(8px) scale(0.985);
    }
  }
</style>
