# The one place that pins Prism's FFmpeg: the source release, its checksum and every component
# the build enables. Sourced by build-ffmpeg.sh, build-ffmpeg-windows.sh and check-ffmpeg.sh.
# Changing this file changes the CI cache key, so the sidecars are rebuilt.

FFMPEG_VERSION="8.1.3"
FFMPEG_URL="https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz"
# Checked against the release signature (key FCF986EA15E6E293A5644F10B4322F04D67658D8) once,
# when the version was pinned.
FFMPEG_SHA256="7138d28c96d9d3e3af4ee3d8cad72741f8ffb40da90c1112235dea3ecd3178a3"

# Spec 5.2 formats plus radio: MP3, AAC/M4A/ALAC, FLAC, WAV/W64, AIFF/CAF, Ogg (Vorbis, Opus,
# FLAC), WMA, APE, WavPack, Musepack, TTA, DSD (DSF, DFF), MKA/WebM, AC-3/E-AC-3, LATM AAC,
# and HLS (TS, raw AAC, fMP4 segments).
FFMPEG_DEMUXERS="aac ac3 aiff ape asf caf dsf eac3 flac hls iff loas matroska mov mp3 mpc mpc8 mpegts ogg tta w64 wav wv"
FFMPEG_DECODERS="aac aac_latm ac3 adpcm_ima_qt adpcm_ima_wav adpcm_ms alac ape dsd_lsbf dsd_lsbf_planar dsd_msbf dsd_msbf_planar dst eac3 flac mp1float mp2float mp3float mpc7 mpc8 opus pcm_alaw pcm_f32be pcm_f32le pcm_f64be pcm_f64le pcm_mulaw pcm_s16be pcm_s16le pcm_s24be pcm_s24le pcm_s32be pcm_s32le pcm_s8 pcm_u8 tta vorbis wavpack wmalossless wmapro wmav1 wmav2 wmavoice"
FFMPEG_PARSERS="aac aac_latm ac3 flac mpegaudio opus vorbis"
# file: local files (`file:<path>`); pipe: streams fed through stdin and the output on stdout;
# the rest is what audio/ffmpeg.rs whitelists for an HLS station URL.
FFMPEG_PROTOCOLS="crypto file http httpproxy https pipe tcp tls"
# What the ffmpeg CLI inserts for `-ss`, `-ac 2 -ar <rate>` and `-f f32le` (the buffer source
# and sink are always built).
FFMPEG_FILTERS="aformat anull aresample atrim"
# f32le is Prism's output; s16le, wav and null serve the tests and quick manual checks. The raw
# PCM muxers are configured as pcm_<format> but listed (and used with -f) as <format>.
FFMPEG_ENCODERS="pcm_f32le pcm_s16le"
FFMPEG_MUXERS="null pcm_f32le pcm_s16le wav"

# Required in every build (check-ffmpeg.sh). Decoders are checked both ways: none missing and
# none extra, so the binary carries only what is listed above.
FFMPEG_REQUIRED_DEMUXERS="$FFMPEG_DEMUXERS"
FFMPEG_REQUIRED_DECODERS="$FFMPEG_DECODERS"
FFMPEG_REQUIRED_PROTOCOLS="$FFMPEG_PROTOCOLS"
FFMPEG_REQUIRED_MUXERS="f32le"
FFMPEG_REQUIRED_ENCODERS="pcm_f32le"
FFMPEG_REQUIRED_FILTERS="aformat aresample atrim"

ffmpeg_commas() {
  echo "$1" | tr ' ' ','
}

# The configure flags shared by every platform. No --enable-gpl, --enable-nonfree or
# --enable-version3: the result is LGPL 2.1 or later.
ffmpeg_common_flags() {
  echo --disable-everything --disable-autodetect \
    --disable-programs --enable-ffmpeg --enable-ffprobe \
    --disable-doc --disable-debug --disable-shared --enable-static \
    --disable-avdevice --disable-swscale --enable-avfilter --enable-swresample \
    --disable-x86asm --optflags=-O2 \
    --enable-demuxer="$(ffmpeg_commas "$FFMPEG_DEMUXERS")" \
    --enable-decoder="$(ffmpeg_commas "$FFMPEG_DECODERS")" \
    --enable-parser="$(ffmpeg_commas "$FFMPEG_PARSERS")" \
    --enable-protocol="$(ffmpeg_commas "$FFMPEG_PROTOCOLS")" \
    --enable-filter="$(ffmpeg_commas "$FFMPEG_FILTERS")" \
    --enable-encoder="$(ffmpeg_commas "$FFMPEG_ENCODERS")" \
    --enable-muxer="$(ffmpeg_commas "$FFMPEG_MUXERS")"
}

# Downloads the pinned tarball into $1 (once) and unpacks it into $2, checking the SHA-256.
ffmpeg_fetch_source() {
  local dl_dir="$1" src_parent="$2"
  local tarball="$dl_dir/ffmpeg-${FFMPEG_VERSION}.tar.xz"
  mkdir -p "$dl_dir" "$src_parent"
  if [ ! -f "$tarball" ]; then
    curl -fsSL --proto '=https' --proto-redir '=https' --retry 3 -o "$tarball.part" "$FFMPEG_URL"
    mv "$tarball.part" "$tarball"
  fi
  local actual
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$tarball" | cut -d' ' -f1)"
  else
    actual="$(shasum -a 256 "$tarball" | cut -d' ' -f1)"
  fi
  if [ "$actual" != "$FFMPEG_SHA256" ]; then
    echo "checksum mismatch for $tarball: $actual" >&2
    rm -f "$tarball"
    exit 1
  fi
  rm -rf "$src_parent/ffmpeg-${FFMPEG_VERSION}"
  tar -xJf "$tarball" -C "$src_parent"
}

# Fails when a listed component does not exist in the source at $1: configure skips unknown
# names silently.
ffmpeg_validate_components() {
  local src="$1" kind list name known
  for kind in demuxer decoder parser protocol filter encoder muxer; do
    case "$kind" in
      demuxer) list="$FFMPEG_DEMUXERS" ;;
      decoder) list="$FFMPEG_DECODERS" ;;
      parser) list="$FFMPEG_PARSERS" ;;
      protocol) list="$FFMPEG_PROTOCOLS" ;;
      filter) list="$FFMPEG_FILTERS" ;;
      encoder) list="$FFMPEG_ENCODERS" ;;
      muxer) list="$FFMPEG_MUXERS" ;;
    esac
    known="$("$src/configure" "--list-${kind}s" | tr -s ' \t' '\n')"
    for name in $list; do
      echo "$known" | grep -qx "$name" || { echo "unknown FFmpeg $kind: $name" >&2; exit 1; }
    done
  done
}

ffmpeg_jobs() {
  nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4
}
