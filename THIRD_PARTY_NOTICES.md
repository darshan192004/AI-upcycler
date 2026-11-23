# Third-Party Notices

AI Upcycler itself is MIT-licensed (see `LICENSE`). The desktop installers also
ship or depend on the components below, which keep their own licenses.

## Android SDK Platform-Tools

- **Component:** `adb` and its Windows helper libraries (`AdbWinApi.dll`,
  `AdbWinUsbApi.dll`) from the Android SDK Platform-Tools package.
- **Copyright:** Copyright (C) 2007-2026 The Android Open Source Project.
- **License:** Apache License, Version 2.0
  (<https://www.apache.org/licenses/LICENSE-2.0>).
- **How it is included:** the binaries are Apache-2.0 and platform-specific, so
  they are not committed. `scripts/fetch-platform-tools.ps1` (Windows) and
  `scripts/fetch-platform-tools.sh` (macOS) download the official package from
  `https://dl.google.com/android/repository/platform-tools-latest-*.zip` at
  build time into `src-tauri/resources/platform-tools/`, which the Tauri bundler
  copies into the installer as `platform-tools/`. The package's own `NOTICE.txt`
  is copied with it and is the authoritative attribution text.
- **Linux:** the release does not bundle platform-tools; install the distro
  package (`android-tools-adb` on Debian/Ubuntu, `android-tools` on Fedora) or
  set `AI_UPCYCLER_ADB`. See `docs/tasks.md` section 2.

## Other dependencies

The Rust crates, the Android worker's Gradle dependencies, and the frontend
packages each declare their licenses in their own package metadata. This file
records the components that ship inside an installer and would otherwise be
missing an attribution.
