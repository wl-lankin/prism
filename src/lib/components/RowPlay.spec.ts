import source from './RowPlay.svelte?raw';
import { compile } from 'svelte/compiler';
import { expect, it } from 'vitest';

// jsdom does not hit-test, so this checks the stylesheet itself. The number (or equalizer) and
// the play button share one grid cell. Fading the number out with opacity gives it its own
// stacking context, which paints it above the button: without `pointer-events: none` the
// invisible number takes every click on the middle of the button, the play icon.
const css = compile(source, { filename: 'RowPlay.svelte', css: 'external' }).css!.code;

/** The declarations of the rules whose selector matches `pattern`. */
function declarations(pattern: RegExp): string {
  return [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .filter(([, selector]) => pattern.test(selector))
    .map(([, , body]) => body)
    .join(';');
}

it('lets clicks through the hidden number to the play button', () => {
  const shownMark = declarations(/\.shown(\.[\w-]+)*\s+\.mark/);
  expect(shownMark).toMatch(/opacity:\s*0/);
  expect(shownMark).toMatch(/pointer-events:\s*none/);
});
