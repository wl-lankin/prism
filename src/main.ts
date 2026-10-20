import '@fontsource-variable/inter';
import '@fontsource-variable/space-grotesk';
import './lib/theme.css';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { mount } from 'svelte';
import App from './App.svelte';
import MiniPlayer from './lib/components/MiniPlayer.svelte';
import { MINI_LABEL } from './lib/ipc';

/** Both windows load this page; the mini window gets the mini player. */
function label(): string {
  try {
    return getCurrentWindow().label;
  } catch {
    return 'main';
  }
}

const target = document.getElementById('app')!;
if (label() === MINI_LABEL) mount(MiniPlayer, { target });
else mount(App, { target });
