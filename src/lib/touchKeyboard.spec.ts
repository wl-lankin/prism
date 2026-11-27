import { expect, it } from 'vitest';
import { isEditable, layoutFor } from './touchKeyboard';

const make = (html: string) => {
  const host = document.createElement('div');
  host.innerHTML = html;
  return host.firstElementChild;
};

it('counts text-like fields as editable and the rest not', () => {
  expect(isEditable(make('<input>'))).toBe(true);
  expect(isEditable(make('<input type="password">'))).toBe(true);
  expect(isEditable(make('<textarea></textarea>'))).toBe(true);
  expect(isEditable(make('<input type="range">'))).toBe(false);
  expect(isEditable(make('<input type="text" disabled>'))).toBe(false);
  expect(isEditable(make('<button></button>'))).toBe(false);
  expect(isEditable(null)).toBe(false);
});

it('lays out the German letters in QWERTZ order with umlauts', () => {
  const rows = layoutFor('letters', 'text');
  const chars = rows.flat().flatMap((k) => (k.kind === 'char' ? [k.value] : []));
  expect(chars.join('')).toContain('qwertzuiopüasdfghjklöäyxcvbnmß');
});

it('keeps ß out of the shifted letters (uppercase would type two characters)', () => {
  const keys = layoutFor('letters', 'text').flat();
  const sharp = keys.find((k) => k.kind === 'char' && k.value === 'ß');
  expect(sharp).toBeDefined();
  expect(sharp?.kind === 'char' && sharp.letter).toBeFalsy();
});
