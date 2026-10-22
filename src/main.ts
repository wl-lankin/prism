import '@fontsource-variable/inter';
import '@fontsource-variable/space-grotesk';
import './lib/theme.css';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { mount } from 'svelte';
import { MINI_LABEL } from './lib/ipc';

/** Both windows load this page; each imports only its own component (and its CSS). */
function label(): string {
  try {
    return getCurrentWindow().label;
  } catch {
    return 'main';
  }
}

const target = document.getElementById('app')!;
const { default: root } =
  label() === MINI_LABEL ? await import('./lib/components/MiniPlayer.svelte') : await import('./App.svelte');
mount(root, { target });
