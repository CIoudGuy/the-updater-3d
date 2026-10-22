# The Updater 3D

A fast Windows app that keeps creative software up to date in one place:

- **Adobe Creative Cloud apps** (Photoshop, Illustrator, Substance 3D and the rest)
- **Blender**, every installed version side by side, plus betas
- **The Craft apps** from ArtCraft (PhotoCraft, VectorCraft, FilmCraft and the others)

It finds what is installed, shows what can be updated with release notes, and installs, updates,
rolls back or removes apps. A tiny background checker starts with Windows and sends a notification
when something can be updated.

Built with [Tauri 2](https://tauri.app): a Rust backend and a Svelte 5 interface.

## How it works

**Adobe.** Installed apps come from Adobe's own install database
(`Common Files\Adobe\caps\hdpim.db`), checked against the files on disk, so leftovers of deleted
apps are not listed. Latest versions come from the public Creative Cloud product catalog. Installs
download the packages from Adobe's CDN and run Adobe's own installer (`HDBox\Setup.exe`) with a
driver file, the way Adobe's admin packages install. If that fails, updates fall back to Adobe's
Remote Update Manager. Creative Cloud stays installed for sign-in and licensing.

**Blender.** Each installed release series (4.2, 4.5, 5.2...) is its own entry. New series install
next to the old ones; bug-fix releases update their series in place, or install as a separate
portable copy when "Keep bug-fix versions" is on. Releases come from download.blender.org with
SHA-256 checks, betas from builder.blender.org, and fix lists from blender.org.

**Craft apps.** Versions come from the ArtCraft release feed and GitHub Releases, installed from
each release's MSI with SHA-256 checks.

**Admin rights.** The app itself runs without them. When an install needs them, it starts a copy of
itself elevated (one UAC prompt per batch) and talks to it over a private named pipe. "Fewer admin
prompts" registers a scheduled task so there is no prompt at all.

**Background.** At sign-in the app starts with `--minimized`: only the Rust checker and the tray
icon run, with no web view. The window is created when opened and destroyed when closed.

## Build

Requirements: Windows 10 or 11, [Rust](https://rustup.rs) 1.90 or newer, Node.js 20 or newer.

```bash
npm install
npx tauri build
```

The installer is written to `src-tauri/target/release/bundle/nsis/`.

### Self-updates (optional)

The app updates itself from a JSON release feed. Both settings are read at build time:

- `src-tauri/.update-feed`: the feed address (the official releases)
- `src-tauri/.update-key`: the key for that feed (sent as `X-Update-Key`)

`UPDATER3D_FEED_URL` and `UPDATER3D_UPDATE_KEY` environment variables override the files, so a
fork can point at its own feed. Without a feed address the app never offers self-updates. The feed answers with:

```json
{
  "app": {
    "channels": {
      "stable": {
        "version": "1.2.0",
        "notes": "What changed",
        "files": [
          { "name": "The Updater 3D_1.2.0_x64-setup.exe", "platform": "windows-x64", "size": 5726826, "sha256": "...", "url": "https://..." }
        ]
      }
    }
  }
}
```

## Develop

```bash
npm run dev            # interface only, with mock data, at http://localhost:1420
npx tauri dev          # the whole app
```

The mock accepts `?state=updating|current|first|offline|blocked|failed|lowdisk|ccdown|signedout`.

## Tests

```bash
cd src-tauri
cargo test                                              # unit and engine tests
cargo test --test engine -- --include-ignored           # downloads: resume, pause, limit, checksum
cargo test --test live_probe -- --ignored --nocapture   # detection on this PC and live feeds
```

## Icon

`src-tauri/icons/make-icon.py <artwork.png> app-icon-1024.png` rebuilds the icon from source
artwork, then `npx tauri icon src-tauri/icons/app-icon-1024.png` makes the other sizes.

## Not affiliated

Adobe, Creative Cloud, Photoshop and Substance are trademarks of Adobe. Blender is a trademark of
the Blender Foundation. This project is not affiliated with either, nor with ArtCraft.

## Made with AI

This project was built with the help of AI coding tools.
