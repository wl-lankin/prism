import { flushSync, mount, unmount } from 'svelte';
import { expect, it } from 'vitest';
import App from './App.svelte';

it('mounts the app shell', () => {
  const target = document.createElement('div');
  const app = mount(App, { target });
  flushSync();
  expect(target.querySelector('h1')?.textContent).toBe('Prism');
  unmount(app);
});
