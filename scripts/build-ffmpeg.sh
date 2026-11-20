#!/usr/bin/env bash
set -euo pipefail

output=${1:?Pass an output directory}
target=${2:-native}
mkdir -p "$output"
output="$(cd "$output" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf -- "$work"' EXIT
cd "$work"

version=8.1.1
checksum=b6863adde98898f42602017462871b5f6333e65aec803fdd7a6308639c52edf3
curl --fail --silent --show-error --location --retry 3 \
  "https://ffmpeg.org/releases/ffmpeg-$version.tar.xz" -o source.tar.xz
printf '%s  %s\n' "$checksum" source.tar.xz | shasum -a 256 -c
tar -xf source.tar.xz
cd "ffmpeg-$version"

options=()
suffix=
if [[ "$target" == windows ]]; then
  options+=(--enable-cross-compile --target-os=mingw32 --arch=x86_64
    --cross-prefix=x86_64-w64-mingw32- --cc=x86_64-w64-mingw32-gcc-posix
    --extra-ldflags=-static)
  suffix=.exe
fi

./configure "${options[@]}" --disable-autodetect --disable-doc \
  --disable-debug --disable-network --disable-avdevice --disable-ffplay \
  --disable-encoders --enable-encoder=rawvideo,mjpeg \
  --disable-muxers --enable-muxer=rawvideo,image2 \
  --disable-demuxers --enable-demuxer=mov,matroska \
  --disable-protocols --enable-protocol=file,pipe
make -j"$(getconf _NPROCESSORS_ONLN)" ffmpeg"$suffix" ffprobe"$suffix"
cp ffmpeg"$suffix" ffprobe"$suffix" "$output/"
