// On-screen keyboard and a Cancel bar for Google's sign-in window in the touchscreen kiosk.
// Prism's own Svelte keyboard only exists inside Prism's page, but the sign-in opens a page we
// do not control (accounts.google.com), where a kiosk user has neither a keyboard nor a way
// back. The Rust side injects this file as an initialization script, in kiosk mode only. It
// follows src/lib/touchKeyboard.ts and TouchKeyboard.svelte: same layouts, same editing.
//
// Hard rules: what is typed is never read, stored, logged or sent anywhere (no network, no
// storage, no globals holding text), and there is no IPC. Everything lives in one host element
// with a closed shadow root, so the page's CSS cannot reach it and ours cannot leak out. The
// only way out of the window is a navigation to a URL that Rust intercepts.
(function () {
  'use strict';

  if (window.__prismOsk) return;
  // Not enumerable, so page scripts that list window properties do not trip over it.
  Object.defineProperty(window, '__prismOsk', { value: true, configurable: true });

  // Tests set this before evaluation to open the shadow root and to stub the navigation. It is
  // read exactly once here and is not consulted again, so it does nothing in production.
  var hook = window.__prismOskTest || null;
  // Rust sets this for the sign-in window only. The same script also runs inside the YouTube
  // Music view, where there is no Cancel bar and the page must not be pushed down. Read once.
  var withBar = window.__prismOskBar === true;

  var CLOSE_URL = 'https://prism.invalid/close-sign-in';
  var BAR_HEIGHT = 44;
  var FALLBACK_HEIGHT = 250;
  var DOUBLE_TAP_MS = 350;
  var REPEAT_START_MS = 400;
  var REPEAT_EVERY_MS = 60;
  var Z = '2147483647';
  var TEXT_TYPES = ['text', 'search', 'url', 'email', 'password', 'tel', 'number', ''];

  var de = String(navigator.language || '').toLowerCase().indexOf('de') === 0;
  var T = de
    ? { cancel: 'Abbrechen', done: 'Fertig', space: 'Leertaste', backspace: 'Löschen', shift: 'Umschalt', enter: 'Eingabe', symbols: 'Zahlen und Zeichen', letters: 'Buchstaben', keyboard: 'Tastatur' }
    : { cancel: 'Cancel', done: 'Done', space: 'Space', backspace: 'Backspace', shift: 'Shift', enter: 'Enter', symbols: 'Numbers and symbols', letters: 'Letters', keyboard: 'Keyboard' };

  var navigate = hook && typeof hook.navigate === 'function'
    ? hook.navigate
    : function (url) { window.location.href = url; };

  // Closed, so the page cannot reach into the keyboard (and read key labels or fake presses).
  var host = document.createElement('prism-osk');
  // The page's stylesheets match elements, not our shadow tree, but they can still match the
  // host itself, so its layout is pinned inline and made immune with !important.
  host.style.cssText = 'all: initial !important; display: block !important; position: fixed !important; top: 0 !important; left: 0 !important; width: 0 !important; height: 0 !important; z-index: ' + Z + ' !important;';
  var shadow = host.attachShadow({ mode: hook ? 'open' : 'closed' });
  if (hook) hook.root = shadow;

  var css = document.createElement('style');
  css.textContent = [
    '*, *::before, *::after { box-sizing: border-box; }',
    // Solid and effect free: the Pi\'s GPU pays a full screen copy for every blur, shadow or transition.
    '.bar { position: fixed; top: 0; left: 0; right: 0; height: ' + BAR_HEIGHT + 'px; z-index: ' + Z + '; display: flex; align-items: center; padding: 0 6px; background: #0d0d10; border-bottom: 1px solid rgba(255,255,255,0.08); font: 17px system-ui, sans-serif; }',
    '.cancel { height: 36px; padding: 0 16px; border: 0; border-radius: 8px; background: #1f1f25; color: #f3f3f5; font: inherit; cursor: default; -webkit-tap-highlight-color: transparent; }',
    '.cancel:active { background: #2e2e37; }',
    '.osk { position: fixed; left: 0; right: 0; bottom: 0; z-index: ' + Z + '; height: ' + FALLBACK_HEIGHT + 'px; display: none; flex-direction: column; gap: 4px; padding: 6px; background: #0d0d10; border-top: 1px solid rgba(255,255,255,0.08); font-family: system-ui, sans-serif; user-select: none; -webkit-user-select: none; touch-action: manipulation; }',
    '.osk.shown { display: flex; }',
    '.row { flex: 1; min-height: 44px; display: flex; gap: 4px; }',
    '.key { flex: 1; min-width: 0; padding: 0; border: 0; border-radius: 8px; background: #1f1f25; color: #f3f3f5; font: inherit; font-size: 20px; cursor: default; -webkit-tap-highlight-color: transparent; }',
    '.key:active { background: #2e2e37; }',
    '.key.special { background: #141418; color: rgba(243,243,245,0.62); font-size: 17px; }',
    '.key.special:active { background: #1f1f25; }',
    '.key.on { color: #f3f3f5; background: #1f1f25; }',
    '.key.lock { color: #0d0d10; background: #f3f3f5; }',
  ].join('\n');
  shadow.appendChild(css);

  // The bar is part of the page layout (padding on the root element, see attach) rather than an
  // overlay, so Google's own header and form are pushed down instead of being covered.
  if (withBar) {
    var bar = document.createElement('div');
    bar.className = 'bar';
    var cancel = document.createElement('button');
    cancel.type = 'button';
    cancel.className = 'cancel';
    cancel.textContent = T.cancel;
    cancel.addEventListener('click', function () { navigate(CLOSE_URL); });
    bar.appendChild(cancel);
    shadow.appendChild(bar);
  }

  var kb = document.createElement('div');
  kb.className = 'osk';
  kb.setAttribute('role', 'group');
  kb.setAttribute('aria-label', T.keyboard);
  shadow.appendChild(kb);

  // State. The field is a reference to the element, never its content.
  var field = null;
  var layer = 'letters';
  var shift = 'off'; // 'off' | 'once' | 'lock'
  var lastShiftTap = 0;
  var repeatStart = 0;
  var repeatTick = 0;
  var keyEls = [];

  function chars(s, letter) {
    return Array.from(s).map(function (v) { return { kind: 'char', value: v, letter: !!letter }; });
  }

  function layoutFor(l, type) {
    var extras = type === 'url' ? ['/', '.'] : type === 'email' ? ['@', '.'] : [',', '.'];
    var bottom = [
      { kind: 'layer', w: 1.6 },
      { kind: 'char', value: extras[0] },
      { kind: 'space', w: 4.4 },
      { kind: 'char', value: extras[1] },
      { kind: 'enter', w: 1.6 },
      { kind: 'done', w: 1.6 },
    ];
    if (l === 'symbols') {
      return [
        chars('1234567890'),
        chars('@.,-_/:;!?'),
        chars('&=+#*\'"()%'),
        chars('$€~').concat([{ kind: 'backspace', w: 1.5 }]),
        bottom,
      ];
    }
    return [
      chars('qwertzuiopü', true),
      chars('asdfghjklöä', true),
      // ß is no letter here: shifted, toUpperCase would type "SS", two characters for one key.
      [{ kind: 'shift', w: 1.5 }].concat(chars('yxcvbnm', true), chars('ß'), [{ kind: 'backspace', w: 1.5 }]),
      bottom,
    ];
  }

  function glyph(key) {
    switch (key.kind) {
      case 'char': return key.letter && shift !== 'off' ? key.value.toUpperCase() : key.value;
      case 'shift': return '⇧';
      case 'backspace': return '⌫';
      case 'enter': return '↵';
      case 'layer': return layer === 'letters' ? '?123' : 'ABC';
      case 'done': return T.done;
      default: return '';
    }
  }

  function ariaLabel(key) {
    switch (key.kind) {
      case 'layer': return layer === 'letters' ? T.symbols : T.letters;
      case 'space': return T.space;
      case 'backspace': return T.backspace;
      case 'shift': return T.shift;
      case 'enter': return T.enter;
      case 'done': return T.done;
      default: return null;
    }
  }

  // Labels and shift state change often, so they are updated in place. The buttons themselves are
  // only rebuilt when the layout changes, because a button replaced under a held finger would
  // never see its pointerup.
  function refresh() {
    keyEls.forEach(function (entry) {
      var key = entry.key;
      var el = entry.el;
      el.textContent = glyph(key);
      var label = ariaLabel(key);
      if (label) el.setAttribute('aria-label', label); else el.removeAttribute('aria-label');
      el.classList.toggle('on', key.kind === 'shift' && shift === 'once');
      el.classList.toggle('lock', key.kind === 'shift' && shift === 'lock');
      if (key.kind === 'shift') el.setAttribute('aria-pressed', String(shift !== 'off'));
    });
  }

  function build() {
    kb.textContent = '';
    keyEls = [];
    layoutFor(layer, field ? field.type || '' : '').forEach(function (row) {
      var rowEl = document.createElement('div');
      rowEl.className = 'row';
      row.forEach(function (key) {
        var el = document.createElement('button');
        el.type = 'button';
        el.tabIndex = -1;
        el.className = 'key' + (key.kind !== 'char' && key.kind !== 'space' ? ' special' : '');
        el.style.flex = String(key.w || 1);
        // Keys must never take focus, or the field would blur and the keyboard would vanish.
        el.addEventListener('mousedown', function (e) { e.preventDefault(); });
        el.addEventListener('pointerdown', function (e) { e.preventDefault(); press(key); });
        el.addEventListener('pointerup', stopRepeat);
        el.addEventListener('pointercancel', stopRepeat);
        el.addEventListener('pointerleave', stopRepeat);
        rowEl.appendChild(el);
        keyEls.push({ key: key, el: el });
      });
      kb.appendChild(rowEl);
    });
    refresh();
  }

  function stopRepeat() {
    clearTimeout(repeatStart);
    clearInterval(repeatTick);
    repeatStart = repeatTick = 0;
  }

  // Editing helpers, as in touchKeyboard.ts.
  function hasSelection(f) {
    try {
      return f.selectionStart !== null && f.selectionEnd !== null;
    } catch (e) {
      return false;
    }
  }

  function fire(f, inputType, data) {
    f.dispatchEvent(new InputEvent('input', { bubbles: true, composed: true, inputType: inputType, data: data }));
  }

  // execCommand is deprecated but is the only way to get a trusted-looking input event and the
  // undo stack in WebKit. On a page we do not control it may be missing or return false, which
  // is why every call has a manual fallback.
  function exec(command, value) {
    try {
      return typeof document.execCommand === 'function' && document.execCommand(command, false, value) === true;
    } catch (e) {
      return false;
    }
  }

  function insertText(f, text) {
    if (exec('insertText', text)) return;
    if (hasSelection(f)) f.setRangeText(text, f.selectionStart, f.selectionEnd, 'end');
    else f.value += text;
    fire(f, 'insertText', text);
  }

  function deleteBackward(f) {
    if (exec('delete')) return;
    if (hasSelection(f)) {
      var start = f.selectionStart;
      var end = f.selectionEnd;
      if (start === end) {
        if (start === 0) return;
        f.setRangeText('', start - 1, end, 'end');
      } else {
        f.setRangeText('', start, end, 'end');
      }
    } else {
      if (!f.value) return;
      f.value = f.value.slice(0, -1);
    }
    fire(f, 'deleteContentBackward', null);
  }

  function keyEvent(type) {
    var e = new KeyboardEvent(type, { key: 'Enter', code: 'Enter', bubbles: true, cancelable: true, composed: true });
    // Some engines ignore keyCode in the init dictionary, and pages still read it.
    if (e.keyCode !== 13) {
      try {
        Object.defineProperty(e, 'keyCode', { get: function () { return 13; } });
        Object.defineProperty(e, 'which', { get: function () { return 13; } });
      } catch (err) { /* the event keeps its key and code */ }
    }
    return e;
  }

  function pressEnter(f) {
    if (f.tagName === 'TEXTAREA') {
      insertText(f, '\n');
      return;
    }
    var handled = !f.dispatchEvent(keyEvent('keydown'));
    f.dispatchEvent(keyEvent('keyup'));
    if (!handled && f.form && typeof f.form.requestSubmit === 'function') f.form.requestSubmit();
  }

  function press(key) {
    if (!field) return;
    switch (key.kind) {
      case 'char':
        typeText(key.letter && shift !== 'off' ? key.value.toUpperCase() : key.value);
        break;
      case 'space':
        typeText(' ');
        break;
      case 'shift': {
        var now = Date.now();
        if (shift === 'lock') shift = 'off';
        else if (shift === 'once' && now - lastShiftTap < DOUBLE_TAP_MS) shift = 'lock';
        else shift = shift === 'once' ? 'off' : 'once';
        lastShiftTap = now;
        refresh();
        break;
      }
      case 'layer':
        layer = layer === 'letters' ? 'symbols' : 'letters';
        build();
        break;
      case 'enter':
        pressEnter(field);
        break;
      case 'backspace': {
        var target = field;
        deleteBackward(target);
        stopRepeat();
        repeatStart = setTimeout(function () {
          repeatTick = setInterval(function () { deleteBackward(target); }, REPEAT_EVERY_MS);
        }, REPEAT_START_MS);
        break;
      }
      case 'done': {
        var done = field;
        hide();
        done.blur();
        break;
      }
    }
  }

  function typeText(text) {
    insertText(field, text);
    if (shift === 'once') {
      shift = 'off';
      refresh();
    }
  }

  // Which elements count as editable: the same set as isEditable in touchKeyboard.ts.
  function isEditable(el) {
    if (!el || el.nodeType !== 1) return false;
    if (el.tagName === 'TEXTAREA') return !el.readOnly && !el.disabled;
    if (el.tagName === 'INPUT') return TEXT_TYPES.indexOf(el.type) !== -1 && !el.readOnly && !el.disabled;
    return false;
  }

  // Google's forms may sit in shadow roots, where the document sees only the outermost host.
  function deepActive() {
    var el = document.activeElement;
    while (el && el.shadowRoot && el.shadowRoot.activeElement) el = el.shadowRoot.activeElement;
    return el;
  }

  // The root element pushes the page up by the bar, and by the keyboard while it is shown.
  // Padding (not margin on body) is used because it also moves content that is positioned
  // relative to the root, and it needs no restoring: the window closes after the sign-in.
  function show(next) {
    if (next !== field) {
      // A new field starts on the letters with shift off, like a phone keyboard.
      layer = next.type === 'number' || next.type === 'tel' ? 'symbols' : 'letters';
      shift = 'off';
      field = next;
      build();
    }
    field = next;
    kb.classList.add('shown');
    var current = field;
    // Measure after the keyboard has been laid out.
    setTimeout(function () {
      if (field !== current) return;
      var height = kb.offsetHeight || FALLBACK_HEIGHT;
      document.documentElement.style.paddingBottom = height + 'px';
      if (current.getBoundingClientRect().bottom > window.innerHeight - height) {
        try { current.scrollIntoView({ block: 'center' }); } catch (e) { /* not scrollable */ }
      }
    }, 0);
  }

  function hide() {
    stopRepeat();
    field = null;
    kb.classList.remove('shown');
    document.documentElement.style.paddingBottom = '';
  }

  document.addEventListener('focusin', function (e) {
    var path = typeof e.composedPath === 'function' ? e.composedPath() : [];
    var target = path.length ? path[0] : e.target;
    if (isEditable(target)) show(target);
    else hide();
  }, true);

  // A tap on the bare background blurs the field without focusing anything, so look again once
  // focus has settled.
  document.addEventListener('focusout', function (e) {
    if (e.relatedTarget) return;
    setTimeout(function () {
      if (field && !isEditable(deepActive())) hide();
    }, 0);
  }, true);

  function attach() {
    if (withBar) document.documentElement.style.paddingTop = BAR_HEIGHT + 'px';
    if (!host.isConnected) document.documentElement.appendChild(host);
  }

  // Re-attach if the page removes our host, and hide when the field leaves the page.
  var observer = new MutationObserver(function () {
    if (!host.isConnected && document.body) document.documentElement.appendChild(host);
    if (field && !field.isConnected) hide();
  });
  observer.observe(document.documentElement, { childList: true, subtree: true });
  // Tests evaluate the script many times in one document and stop each instance again.
  if (hook) hook.observer = observer;

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', attach, { once: true });
  else attach();
})();
