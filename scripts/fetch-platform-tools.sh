#!/usr/bin/env sh
# Downloads Android platform-tools for macOS into the Tauri resource dir.
#
# The binaries are Apache-2.0 and platform-specific, so they are fetched at
# build time instead of being committed (issue #125). macOS keeps Tauri
# resources in `Contents/Resources`, which the resolver checks; see
# core/src/adb/binary.rs.
#
# Usage:  scripts/fetch-platform-tools.sh [destination]
set -eu

case "$(uname -s)" in
    Darwin)
        url='https://dl.google.com/android/repository/platform-tools-latest-darwin.zip'
        ;;
    *)
        # Linux: `adb` comes from the distro package (android-tools-adb /
        # android-tools) on PATH; the release workflow does not fetch it.
        echo "platform-tools are only fetched for macOS here; use the distro package on Linux" >&2
        exit 1
        ;;
esac

dest="${1:-$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)/src-tauri/resources/platform-tools}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "Fetching $url"
curl -fsSL "$url" -o "$tmp/platform-tools.zip"
unzip -q "$tmp/platform-tools.zip" -d "$tmp"

mkdir -p "$dest"
cp -R "$tmp/platform-tools/." "$dest/"

"$dest/adb" version
echo "platform-tools ready at $dest"
