# FFmpeg in Prism

Prism decodes most formats itself (symphonia). For everything else, for HLS radio and for
reading the tags of files its own tag reader cannot parse, it runs `ffmpeg` and `ffprobe` as
separate programs. The installers bundle a minimal build of both, made from the unmodified
FFmpeg source by the scripts in this repo.

## License

The bundled FFmpeg is licensed under the **GNU Lesser General Public License, version 2.1 or
later** (LGPL 2.1+). It is configured without `--enable-gpl`, `--enable-nonfree` and
`--enable-version3`, so no GPL or non-free component is part of it. Prism only runs the two
programs; it does not link FFmpeg.

## Source

- Version: **FFmpeg 8.1.3**
- Source: <https://ffmpeg.org/releases/ffmpeg-8.1.3.tar.xz>
  (signature: <https://ffmpeg.org/releases/ffmpeg-8.1.3.tar.xz.asc>, FFmpeg release key
  `FCF986EA15E6E293A5644F10B4322F04D67658D8`)
- SHA-256: `7138d28c96d9d3e3af4ee3d8cad72741f8ffb40da90c1112235dea3ecd3178a3`
- The source is used as released, without patches. The build scripts that turn it into the
  bundled programs are in this repo: `scripts/ffmpeg-config.sh`, `scripts/build-ffmpeg.sh`
  (macOS), `scripts/build-ffmpeg-windows.sh` (Windows).

## Configure line

Shared by both platforms (from `scripts/ffmpeg-config.sh`):

```
--disable-everything --disable-autodetect --disable-programs --enable-ffmpeg --enable-ffprobe
--disable-doc --disable-debug --disable-shared --enable-static --disable-avdevice
--disable-swscale --enable-avfilter --enable-swresample --disable-x86asm --optflags=-O2
--enable-demuxer=aac,ac3,aiff,ape,asf,caf,dsf,eac3,flac,hls,iff,loas,matroska,mov,mp3,mpc,mpc8,mpegts,ogg,tta,w64,wav,wv
--enable-decoder=aac,aac_latm,ac3,adpcm_ima_wav,adpcm_ms,alac,ape,dsd_lsbf,dsd_lsbf_planar,dsd_msbf,dsd_msbf_planar,eac3,flac,mp1float,mp2float,mp3float,mpc7,mpc8,opus,pcm_alaw,pcm_f32be,pcm_f32le,pcm_f64be,pcm_f64le,pcm_mulaw,pcm_s16be,pcm_s16le,pcm_s24be,pcm_s24le,pcm_s32be,pcm_s32le,pcm_s8,pcm_u8,tta,vorbis,wavpack,wmalossless,wmapro,wmav1,wmav2
--enable-parser=aac,aac_latm,ac3,flac,mpegaudio,opus,vorbis
--enable-protocol=crypto,file,http,httpproxy,https,pipe,tcp,tls
--enable-filter=aformat,anull,aresample,atrim
--enable-encoder=pcm_f32le,pcm_s16le
--enable-muxer=null,pcm_f32le,pcm_s16le,wav
```

macOS adds (once per architecture, `arm64` and `x86_64`, joined with `lipo`):

```
--cc=clang --arch=<arch> --target-os=darwin --extra-cflags='-arch <arch> -mmacosx-version-min=11.0'
--extra-ldflags='-arch <arch> -mmacosx-version-min=11.0' --enable-pthreads --enable-securetransport
```

Windows x64 adds:

```
--enable-cross-compile --cross-prefix=x86_64-w64-mingw32- --arch=x86_64 --target-os=mingw32
--extra-ldflags=-static --enable-w32threads --enable-schannel
```

`ffmpeg -version` of a bundled binary prints its exact line.

## What is in it, and why

- **Demuxers and decoders:** the formats in the design spec (5.2): MP3, AAC/M4A, ALAC, FLAC,
  WAV/W64, AIFF/CAF, Ogg Vorbis/Opus/FLAC, WMA (incl. Pro and Lossless), APE, WavPack,
  Musepack, TTA, DSD (DSF, DFF, also DST-compressed), MKA/WebM; for radio also AC-3/E-AC-3, LATM AAC and HLS
  (MPEG-TS, raw AAC and fMP4 segments).
- **Protocols:** `file` and `pipe` for local files and the streams Prism feeds through stdin;
  `http`, `https`, `httpproxy`, `tcp`, `tls` and `crypto` for HLS stations, exactly the
  `-protocol_whitelist` Prism passes. TLS is the system's: Secure Transport on macOS, Schannel
  on Windows.
- **Output:** 32-bit float PCM on stdout (`-f f32le`), resampled with libswresample. The
  `null`, `s16le` and `wav` outputs are for tests and manual checks.
- **Not in it:** video decoders (embedded cover art is read by Prism itself; the ffmpeg
  program always brings a few tiny video filters of its own), any
  encoder beyond raw PCM, devices, network protocols other than the above, hardware
  acceleration, external libraries.

## Building

```
scripts/build-ffmpeg.sh            # macOS: universal ffmpeg + ffprobe
scripts/build-ffmpeg-windows.sh    # Windows x64, cross-compiled with mingw-w64 (Linux or macOS)
scripts/check-ffmpeg.sh <dir>      # LGPL, version and component check, prints sizes
scripts/stage-ffmpeg.sh <dir>      # copies a built pair into src-tauri/binaries/
```

Output goes to `ffmpeg-build/out/universal-apple-darwin/` and
`ffmpeg-build/out/x86_64-pc-windows-msvc/` (both ignored by git), and is staged into
`src-tauri/binaries/` as Tauri sidecars. A release build bundles them with
`--config src-tauri/tauri.sidecar.conf.json`; dev builds bundle nothing and use the FFmpeg on
`PATH` (or `PRISM_FFMPEG` / `PRISM_FFPROBE`).

The Windows build is cross-compiled with mingw-w64 rather than built in MSYS2 on Windows: the
same script runs on a Linux CI runner and on the Mac, FFmpeg's configure runs in seconds
instead of many minutes under MSYS2's process emulation, and no MSYS2 package state is
involved. The result is fully static and imports only Windows system DLLs (the script checks).
`--disable-x86asm` keeps NASM out of both builds; for audio decoding the hand-written x86
assembly saves little CPU time.

When the version or a component changes: update `scripts/ffmpeg-config.sh` and this file
(`scripts/check-ffmpeg.sh` fails when the version or checksum here differs from the config).
