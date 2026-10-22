(function () {
  'use strict';

  var allowDev = typeof ALLOW_DEV_HOST !== 'undefined' && ALLOW_DEV_HOST === true;
  var hostOk = location.host === 'music.youtube.com' || (allowDev && location.hostname === '127.0.0.1');
  if (!hostOk || window.top !== window || window.__prismBridge) return;
  window.__prismBridge = true;

  // The only place that knows YouTube Music's DOM. A missing entry makes the state unavailable.
  var SEL = {
    bar: 'ytmusic-player-bar',
    toggle: '#play-pause-button',
    next: '.next-button',
    prev: '.previous-button',
    video: ['ytmusic-player video', '#movie_player video'],
    // Queue entries are looked up only inside the player's queue element. YouTube Music may
    // render it only once its player page was opened.
    queue: 'ytmusic-player-queue',
    queueItem: 'ytmusic-player-queue-item',
    queueTwin: '#counterpart-renderer',
    queueTitle: '.song-title',
    queueByline: '.byline',
    queueThumb: 'img',
    queuePlay: 'ytmusic-play-button-renderer',
    // Either mark on an entry makes it the current one.
    queueCurrent: ['[selected]', '[play-button-state="playing"]', '[play-button-state="paused"]'],
  };
  var TEXT_MAX = 256;
  var URL_MAX = 2048;
  var QUEUE_MAX = 200;
  var QUEUE_GAP_MS = 500;
  // Under Prism's cap for a queue report (256 KiB), with room for the report around the items.
  var QUEUE_BYTES_MAX = 255 * 1024;
  var HEARTBEAT_MS = 1000;
  var EVENTS = ['play', 'pause', 'ended', 'durationchange', 'loadedmetadata', 'seeked'];

  // Prism's own transport plays and steers, so the page's player bar is hidden (owner,
  // 2026-10-22). It stays in the DOM: the bridge still reads it and clicks its buttons.
  var HIDE_BAR_CSS =
    'ytmusic-player-bar{visibility:hidden!important;pointer-events:none!important;' +
    'height:0!important;min-height:0!important;overflow:hidden!important;}' +
    'ytmusic-app-layout{--ytmusic-player-bar-height:0px!important;}';

  function hideBar() {
    var root = document.head || document.documentElement;
    if (!root || document.getElementById('prism-hide-bar')) return;
    var style = document.createElement('style');
    style.id = 'prism-hide-bar';
    style.textContent = HIDE_BAR_CSS;
    root.appendChild(style);
  }

  var lastKey = null;
  var watchedVideo = null;
  var watchedBar = null;
  var observer = null;
  var lastQueueKey = null;
  var lastQueueAt = -Infinity;
  var queueTimer = null;
  var watchedQueue = null;
  var queueObserver = null;

  function cap(value, max) {
    return typeof value === 'string' ? value.slice(0, max) : '';
  }

  function find(selector, root) {
    try {
      return (root || document).querySelector(selector);
    } catch (e) {
      return null;
    }
  }

  // Controls are only looked up inside the player bar, never elsewhere in the page.
  function control(selector) {
    var bar = find(SEL.bar);
    return bar ? find(selector, bar) : null;
  }

  // Only the player's own video counts; other <video> elements on the page (previews, ads) are ignored.
  function findVideo() {
    return find(SEL.video[0]) || find(SEL.video[1]);
  }

  function enabled(button) {
    return !!button && !button.disabled && button.getAttribute('aria-disabled') !== 'true' && !button.hasAttribute('disabled');
  }

  function artworkOf(metadata) {
    var list = metadata && metadata.artwork;
    if (!list || typeof list.length !== 'number') return '';
    var best = '';
    var bestArea = -1;
    for (var i = 0; i < list.length; i++) {
      var item = list[i];
      var src = item && typeof item.src === 'string' ? item.src : '';
      if (src.length > URL_MAX || src.slice(0, 8).toLowerCase() !== 'https://') continue;
      var area = 0;
      var m = /^(\d+)x(\d+)/.exec(typeof item.sizes === 'string' ? item.sizes : '');
      if (m) area = Number(m[1]) * Number(m[2]);
      if (area > bestArea) {
        bestArea = area;
        best = src;
      }
    }
    return best;
  }

  function snapshot() {
    var bar = find(SEL.bar);
    var toggle = control(SEL.toggle);
    var next = control(SEL.next);
    var prev = control(SEL.prev);
    var video = findVideo();
    var session = navigator.mediaSession;
    var metadata = session && session.metadata;
    var time = video ? video.currentTime : 0;
    var length = video ? video.duration : 0;
    return {
      v: 1,
      kind: 'state',
      playing: !!video && !video.paused && !video.ended,
      title: cap(metadata && metadata.title, TEXT_MAX),
      artist: cap(metadata && metadata.artist, TEXT_MAX),
      album: cap(metadata && metadata.album, TEXT_MAX),
      artwork: artworkOf(metadata),
      positionMs: Number.isFinite(time) && time > 0 ? Math.round(time * 1000) : 0,
      durationMs: Number.isFinite(length) && length > 0 ? Math.round(length * 1000) : 0,
      canNext: enabled(next),
      canPrev: enabled(prev),
      available: !!(bar && toggle && next && prev && video),
    };
  }

  function send(report) {
    var internals = window.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== 'function') return;
    try {
      var pending = internals.invoke('ytm_report', { report: report });
      if (pending && typeof pending.catch === 'function') pending.catch(function () {});
    } catch (e) {
      // The report channel is best effort.
    }
  }

  // While playing, the position moves on its own and the heartbeat carries it; otherwise a new position is a change.
  function keyOf(state) {
    var copy = {};
    for (var name in state) copy[name] = state[name];
    if (state.playing) copy.positionMs = 0;
    return JSON.stringify(copy);
  }

  function evaluate(heartbeat) {
    refresh();
    var state = snapshot();
    var key = keyOf(state);
    if (key === lastKey && !(heartbeat && state.playing)) return;
    lastKey = key;
    send(state);
  }

  function safeEvaluate() {
    try {
      evaluate(false);
    } catch (e) {
      // Report nothing rather than throw into the page.
    }
  }

  function onVideoEvent(event) {
    try {
      if (event.target !== findVideo()) return;
      if (event.type === 'ended') send({ v: 1, kind: 'ended' });
      evaluate(false);
    } catch (e) {
      // Report nothing rather than throw into the page.
    }
  }

  function refresh() {
    var video = findVideo();
    if (video !== watchedVideo) {
      if (watchedVideo) {
        for (var i = 0; i < EVENTS.length; i++) watchedVideo.removeEventListener(EVENTS[i], onVideoEvent);
      }
      watchedVideo = video;
      if (video) {
        for (var j = 0; j < EVENTS.length; j++) video.addEventListener(EVENTS[j], onVideoEvent);
      }
    }
    var bar = find(SEL.bar);
    if (bar !== watchedBar) {
      if (observer) observer.disconnect();
      watchedBar = bar;
      if (bar && typeof MutationObserver === 'function') {
        observer = observer || new MutationObserver(safeEvaluate);
        observer.observe(bar, { attributes: true, childList: true, subtree: true, characterData: true });
      }
    }
  }

  function queueItems(queue) {
    var items = [];
    if (!queue) return items;
    try {
      var list = queue.querySelectorAll(SEL.queueItem);
      for (var i = 0; i < list.length && items.length < QUEUE_MAX; i++) {
        // A song can sit in the page twice, as its song and its video version; the one
        // under the counterpart is the hidden twin (owner check, 2026-10-22).
        if (list[i].closest && list[i].closest(SEL.queueTwin)) continue;
        items.push(list[i]);
      }
    } catch (e) {
      // A broken queue reads as an empty one.
    }
    return items;
  }

  function textOf(root, selector) {
    var el = find(selector, root);
    var text = el && typeof el.textContent === 'string' ? el.textContent : '';
    return cap(text.replace(/\s+/g, ' ').trim(), TEXT_MAX);
  }

  function httpsUrl(raw) {
    if (typeof raw !== 'string') return '';
    var url = raw.slice(0, 2) === '//' ? 'https:' + raw : raw;
    return url.length <= URL_MAX && url.slice(0, 8).toLowerCase() === 'https://' ? url : '';
  }

  // YouTube Music loads an entry's image only once it scrolls into its own view, but the
  // entry's data already names its thumbnails: the largest of those, else the loaded image.
  function thumbOf(item) {
    try {
      var data = item.data || (item.__data && item.__data.data);
      var list = data && data.thumbnail && data.thumbnail.thumbnails;
      if (Array.isArray(list)) {
        var best = '';
        var bestSize = -1;
        for (var i = 0; i < list.length; i++) {
          var url = httpsUrl(list[i] && list[i].url);
          var size = Number(list[i] && list[i].width) || 0;
          if (url && size > bestSize) {
            best = url;
            bestSize = size;
          }
        }
        if (best) return best;
      }
    } catch (e) {
      // Fall back to the image.
    }
    var img = find(SEL.queueThumb, item);
    return httpsUrl(img && img.src);
  }

  function isCurrent(item) {
    for (var i = 0; i < SEL.queueCurrent.length; i++) {
      try {
        if (item.matches(SEL.queueCurrent[i])) return true;
      } catch (e) {
        // A selector this engine does not know.
      }
    }
    return false;
  }

  function queueSnapshot(queue) {
    var items = queueItems(queue).map(function (item) {
      return {
        title: textOf(item, SEL.queueTitle),
        byline: textOf(item, SEL.queueByline),
        thumb: thumbOf(item),
        current: isCurrent(item),
      };
    });
    return { v: 1, kind: 'queue', items: fit(items), hasQueue: !!queue };
  }

  var encoder = typeof TextEncoder === 'function' ? new TextEncoder() : null;

  function bytes(text) {
    return encoder ? encoder.encode(text).length : text.length * 3;
  }

  // A queue whose report would be larger than Prism takes loses its tail instead of being
  // refused whole.
  function fit(items) {
    var total = 0;
    for (var i = 0; i < items.length; i++) {
      total += bytes(JSON.stringify(items[i])) + 1;
      if (total > QUEUE_BYTES_MAX) return items.slice(0, i);
    }
    return items;
  }

  function watchQueue(queue) {
    if (queue === watchedQueue) return;
    if (queueObserver) queueObserver.disconnect();
    watchedQueue = queue;
    if (queue && typeof MutationObserver === 'function') {
      queueObserver = queueObserver || new MutationObserver(queueSoon);
      queueObserver.observe(queue, { attributes: true, childList: true, subtree: true, characterData: true });
    }
  }

  function checkQueue() {
    queueTimer = null;
    try {
      lastQueueAt = Date.now();
      var queue = find(SEL.queue);
      watchQueue(queue);
      var report = queueSnapshot(queue);
      var key = JSON.stringify(report);
      if (key === lastQueueKey) return;
      lastQueueKey = key;
      send(report);
    } catch (e) {
      // Report nothing rather than throw into the page.
    }
  }

  // The queue is read at most once per QUEUE_GAP_MS, however busy the page is; a change in
  // between is read when the gap is over.
  function queueSoon() {
    if (queueTimer !== null) return;
    var wait = Math.min(QUEUE_GAP_MS, lastQueueAt + QUEUE_GAP_MS - Date.now());
    if (wait <= 0) checkQueue();
    else queueTimer = setTimeout(checkQueue, wait);
  }

  function click(selector) {
    var button = control(selector);
    if (enabled(button)) button.click();
  }

  function isPlaying() {
    var video = findVideo();
    return !!video && !video.paused && !video.ended;
  }

  function number(value) {
    return typeof value === 'number' && Number.isFinite(value) ? value : null;
  }

  function command(action) {
    var video;
    switch (action) {
      case 'toggle':
        click(SEL.toggle);
        break;
      case 'play':
        if (findVideo() && !isPlaying()) click(SEL.toggle);
        break;
      case 'pause':
        if (isPlaying()) click(SEL.toggle);
        break;
      case 'next':
        click(SEL.next);
        break;
      case 'prev':
        click(SEL.prev);
        break;
      case 'seek':
        var ms = number(window.__prismSeekMs);
        video = findVideo();
        if (video && ms !== null && ms >= 0) video.currentTime = ms / 1000;
        break;
      case 'volume':
        var level = number(window.__prismVolume);
        video = findVideo();
        if (video && level !== null) video.volume = Math.min(1, Math.max(0, level));
        break;
      case 'queue':
        var at = window.__prismQueueIndex;
        if (typeof at !== 'number' || !Number.isInteger(at) || at < 0) break;
        var entry = queueItems(find(SEL.queue))[at];
        if (entry) (find(SEL.queuePlay, entry) || entry).click();
        break;
      default:
        break;
    }
  }

  window.addEventListener('prism-ytm', function (event) {
    try {
      command(event && event.detail);
    } catch (e) {
      // A broken page must never break the listener.
    }
  });

  function tick() {
    try {
      // The script runs before the page has a head; the style goes in on the first tick
      // that finds one.
      hideBar();
      evaluate(true);
    } catch (e) {
      // Report nothing rather than throw into the page.
    }
    queueSoon();
  }

  tick();
  setInterval(tick, HEARTBEAT_MS);
})();
