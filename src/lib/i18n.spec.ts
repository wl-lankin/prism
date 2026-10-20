import { afterEach, describe, expect, it } from 'vitest';
import { lang, setLang, strings, systemLang, t, tc } from './i18n';

const placeholders = (s: string) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();

afterEach(() => setLang('en'));

describe('string tables', () => {
  it('has every key in both languages', () => {
    expect(Object.keys(strings.de).sort()).toEqual(Object.keys(strings.en).sort());
  });

  it('uses the same placeholders in both languages', () => {
    for (const key of Object.keys(strings.en)) {
      expect(placeholders(strings.de[key]), key).toEqual(placeholders(strings.en[key]));
    }
  });

  it('contains no en dash or em dash', () => {
    for (const table of [strings.en, strings.de]) {
      for (const [key, value] of Object.entries(table)) {
        expect(/[\u2013\u2014]/.test(value), key).toBe(false);
      }
    }
  });

  it('has no empty strings', () => {
    for (const table of [strings.en, strings.de]) {
      for (const [key, value] of Object.entries(table)) {
        expect(value.trim().length, key).toBeGreaterThan(0);
      }
    }
  });
});

describe('t', () => {
  it('translates into the chosen language', () => {
    setLang('de');
    expect(lang()).toBe('de');
    expect(t('nav.songs')).toBe(strings.de['nav.songs']);
    setLang('en');
    expect(t('nav.songs')).toBe(strings.en['nav.songs']);
  });

  it('fills placeholders', () => {
    expect(t('flow.failed', { message: 'no output device' })).toContain('no output device');
  });

  it('counts with a singular form and grouped digits', () => {
    expect(tc('view.count.albums', 1)).toBe('1 album');
    expect(tc('view.count.albums', 3000)).toBe('3,000 albums');
    setLang('de');
    expect(tc('view.count.albums', 1)).toBe('1 Album');
    expect(tc('view.count.albums', 3000)).toBe('3.000 Alben');
  });

  it('returns the key for an unknown key', () => {
    expect(t('does.not.exist')).toBe('does.not.exist');
  });
});

describe('systemLang', () => {
  it('picks German for any de* language', () => {
    expect(systemLang(['de-AT', 'en-US'])).toBe('de');
    expect(systemLang(['de'])).toBe('de');
  });

  it('takes the first supported language in the list', () => {
    expect(systemLang(['fr-FR', 'de-DE'])).toBe('de');
    expect(systemLang(['en-GB', 'de-DE'])).toBe('en');
  });

  it('falls back to English', () => {
    expect(systemLang(['fr-FR', 'it-IT'])).toBe('en');
    expect(systemLang([])).toBe('en');
  });
});
