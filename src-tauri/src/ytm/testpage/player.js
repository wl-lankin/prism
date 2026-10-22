// The fake YouTube Music player: three demo songs, each a quiet tone the server synthesizes.
// It keeps the shape the bridge reads (a <video> in ytmusic-player, the buttons in
// ytmusic-player-bar, navigator.mediaSession metadata) and behaves like the real player:
// the play button toggles, next and previous switch songs, a song that ends plays the next one.
(function () {
  'use strict';

  var SONGS = [
    { title: 'Demo Song 1', artist: 'Demo Artist 1', album: 'Demo Album 1', color: '#ffc94d' },
    { title: 'Demo Song 2', artist: 'Demo Artist 2', album: 'Demo Album 2', color: '#5fd3c6' },
    { title: 'Demo Song 3', artist: 'Demo Artist 3', album: 'Demo Album 3', color: '#b48cff' },
  ];
  // Previous restarts the song after this many seconds, as on YouTube Music.
  var RESTART_AFTER = 3;

  var video = document.querySelector('ytmusic-player video');
  var toggle = document.getElementById('play-pause-button');
  var next = document.querySelector('ytmusic-player-bar .next-button');
  var prev = document.querySelector('ytmusic-player-bar .previous-button');
  var art = document.getElementById('art');
  var title = document.getElementById('title');
  var artist = document.getElementById('artist');
  var time = document.getElementById('time');
  var queue = document.querySelectorAll('ytmusic-player-queue ytmusic-player-queue-item');
  var index = 0;

  // The bridge only takes https artwork, so Prism drops this one: the fake page shows it itself.
  function artwork(color) {
    var svg =
      '<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96"><rect width="96" height="96" fill="' +
      color +
      '"/></svg>';
    return 'data:image/svg+xml,' + encodeURIComponent(svg);
  }

  function clock(seconds) {
    if (!isFinite(seconds) || seconds < 0) seconds = 0;
    var whole = Math.floor(seconds);
    var rest = whole % 60;
    return Math.floor(whole / 60) + ':' + (rest < 10 ? '0' : '') + rest;
  }

  // Writes only changes, so the bridge's observer on the bar sees no churn.
  function text(element, value) {
    if (element.textContent !== value) element.textContent = value;
  }

  function attribute(element, name, value) {
    if (element.getAttribute(name) !== value) element.setAttribute(name, value);
  }

  function showTime() {
    text(time, clock(video.currentTime) + ' / ' + clock(video.duration));
  }

  function showState() {
    var playing = !video.paused && !video.ended;
    text(toggle, playing ? 'Pause' : 'Play');
    attribute(toggle, 'aria-label', playing ? 'Pause' : 'Play');
    // Like the real player bar: nothing before the first song.
    attribute(prev, 'aria-disabled', index === 0 && video.currentTime <= RESTART_AFTER ? 'true' : 'false');
    if ('mediaSession' in navigator) navigator.mediaSession.playbackState = playing ? 'playing' : 'paused';
  }

  function load(n, play) {
    index = (n + SONGS.length) % SONGS.length;
    var song = SONGS[index];
    video.src = 'tone/' + (index + 1) + '.wav';
    text(title, song.title);
    text(artist, song.artist);
    art.style.background = song.color;
    for (var i = 0; i < queue.length; i++) {
      if (i === index) {
        if (!queue[i].hasAttribute('selected')) queue[i].setAttribute('selected', '');
      } else if (queue[i].hasAttribute('selected')) {
        queue[i].removeAttribute('selected');
      }
    }
    if ('mediaSession' in navigator && typeof MediaMetadata === 'function') {
      navigator.mediaSession.metadata = new MediaMetadata({
        title: song.title,
        artist: song.artist,
        album: song.album,
        artwork: [{ src: artwork(song.color), sizes: '96x96', type: 'image/svg+xml' }],
      });
    }
    if (play) start();
    showState();
    showTime();
  }

  function start() {
    var pending = video.play();
    if (pending && typeof pending.catch === 'function') pending.catch(function () {});
  }

  function onToggle() {
    if (video.paused || video.ended) start();
    else video.pause();
  }

  function onNext() {
    load(index + 1, true);
  }

  function onPrev() {
    if (prev.getAttribute('aria-disabled') === 'true') return;
    if (video.currentTime > RESTART_AFTER) {
      video.currentTime = 0;
      showState();
      return;
    }
    load(index - 1, !video.paused);
  }

  // A click on a queue entry (its play button or the entry) plays that song, as on YouTube Music.
  Array.prototype.forEach.call(queue, function (item, n) {
    var thumb = item.querySelector('img');
    if (thumb) thumb.src = artwork(SONGS[n].color);
    item.addEventListener('click', function () {
      load(n, true);
    });
  });

  toggle.addEventListener('click', onToggle);
  next.addEventListener('click', onNext);
  prev.addEventListener('click', onPrev);
  video.addEventListener('play', showState);
  video.addEventListener('pause', showState);
  video.addEventListener('seeked', showState);
  video.addEventListener('timeupdate', function () {
    showTime();
    showState();
  });
  video.addEventListener('durationchange', showTime);
  video.addEventListener('ended', function () {
    load(index + 1, true);
  });

  if ('mediaSession' in navigator && typeof navigator.mediaSession.setActionHandler === 'function') {
    var handlers = { play: start, pause: function () { video.pause(); }, nexttrack: onNext, previoustrack: onPrev };
    Object.keys(handlers).forEach(function (action) {
      try {
        navigator.mediaSession.setActionHandler(action, handlers[action]);
      } catch (e) {
        // An action this engine does not know.
      }
    });
  }

  load(0, false);
})();
