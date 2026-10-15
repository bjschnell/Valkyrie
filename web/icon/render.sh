#!/bin/sh
# Renders helmet.png (the app icon, cropped to its tile) to the PNGs the manifest,
# iOS and the browser tab want: sh render.sh (needs ImageMagick 7).
set -e
cd "$(dirname "$0")"
out=../public/icons
# 256 colors, dithered: a quarter the size, and they go in the binary.
small="-strip -dither FloydSteinberg -colors 256"
# Rounded like the art's own tile, transparent outside it.
rounded() {
  magick helmet.png -resize "$1x$1" \
    \( -size "$1x$1" xc:black -fill white -draw "roundrectangle 0,0 $(($1 - 1)),$(($1 - 1)) $(($1 * 11 / 100)),$(($1 * 11 / 100))" \) \
    -alpha off -compose CopyOpacity -composite $small "PNG8:$2"
}
rounded 192 $out/icon-192.png
rounded 512 $out/icon-512.png
rounded 64 ../public/favicon.png
# Square and opaque: Android masks it, iOS rounds it.
magick helmet.png -resize 512x512 $small PNG8:$out/maskable-512.png
magick helmet.png -resize 180x180 $small PNG8:$out/apple-touch-icon.png
