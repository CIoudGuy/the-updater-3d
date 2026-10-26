<div align="center">

<img src="docs/icon.png" width="112" alt="The Updater 3D icon">

# The Updater 3D

**Update, install and cleanly uninstall all your Windows apps from one place.**

Adobe Creative Cloud, Blender, the Craft apps, OBS, Krita, Unity and 15,000+ other apps.
Finds what you have, shows what changed, and updates everything in one click.

[**Download for Windows**](https://github.com/CIoudGuy/the-updater-3d/releases/latest)
&nbsp;·&nbsp; [What's new](https://github.com/CIoudGuy/the-updater-3d/releases)
&nbsp;·&nbsp; [Report a bug](https://github.com/CIoudGuy/the-updater-3d/issues)

![Home](docs/home.png)

</div>

## Features

- **One list for everything.** Every installed app, its version, and whether an update is waiting.
  Portable apps count too: a portable browser like Thorium is found through its shortcut and
  updated in place, with its profile kept.
- **Update all in one click.** Downloads run in parallel over several connections; installs run
  silently, one at a time, in the background.
- **Full change lists.** Release notes for every version, straight from each publisher.
- **Safe by default.** Every download is checked against the publisher's checksum or signature
  before anything runs. Nothing is installed that fails the check.
- **Picks up where it left off.** Pause, resume, lost connection or a restart: downloads continue
  instead of starting over.
- **Roll back or keep versions.** Install any older version, or keep Blender, Godot and Unity
  versions side by side. Settings are saved before every update, so a rollback can put back the
  ones the older version had.
- **Stable or pre-release.** Each app that publishes betas or release candidates (and NVIDIA's and
  Intel's beta drivers) can follow them, or stay on final versions. Betas published as their own
  apps, like Chrome Beta or VS Code Insiders, are linked from the app's page.
- **Get new apps.** Browse and search the full catalog, with descriptions, icons and change lists.
- **A better uninstaller.** Every program on the PC, uninstalled cleanly: leftover files, registry
  keys, shortcuts, services and startup entries are shown before anything is deleted, with a
  30-day restore. Also finds broken entries and dead shortcuts.
- **Light and quiet.** A small background checker sends a notification when updates are ready
  (click it to open the app on that page), tuned to your PC so it stays fast on older machines.

<table>
  <tr>
    <td><img src="docs/installed.png" alt="Installed apps"></td>
    <td><img src="docs/getapps.png" alt="Get apps"></td>
  </tr>
  <tr>
    <td align="center">Installed apps</td>
    <td align="center">Get apps</td>
  </tr>
</table>

![Queue](docs/queue.png)

## Supported apps

| Group | Apps | Source |
|---|---|---|
| **Adobe** | Photoshop, Illustrator, Premiere Pro, After Effects, Lightroom, the Substance 3D apps and the rest, Camera Raw and other components, Creative Cloud itself | Adobe's update catalog and Remote Update Manager |
| **Blender** | Every release series side by side, LTS, betas | download.blender.org, builder.blender.org |
| **Craft apps** | ArtCraft and the Craft apps (PhotoCraft, VectorCraft, FilmCraft and more) | ArtCraft's releases |
| **More apps** | OBS Studio, Krita, FreeCAD, Godot, Unity Editor, Unity Hub, Epic Games Launcher (Preview) | Each project's own releases |
| **Other apps** | 15,000+ Windows apps: browsers, chat, dev tools, media, utilities, including app packages (MSIX) like Affinity | Microsoft's public winget catalog |
| **Drivers** | NVIDIA (Game Ready or Studio), AMD Radeon and Radeon PRO, AMD chipset, RAID and Ryzen AI NPU, every Intel device Intel lists (graphics, Wi-Fi, Bluetooth, network, NPU), the BIOS, and every driver Windows Update offers. Drivers install without their makers' extra apps | NVIDIA, AMD and Intel directly; Dell, HP, Lenovo and ASUS lists; Windows Update |
| **Windows** | Security, monthly, .NET, Defender and optional updates, and new Windows versions, installed when you click Update | Windows Update |
| **Everything else** | Any other program on the PC, for uninstalling and cleanup | Windows' list of installed programs |

## Install

1. Download the installer from the [latest release](https://github.com/CIoudGuy/the-updater-3d/releases/latest).
2. Run it. Windows may show "Windows protected your PC" because the installer is not code-signed:
   click **More info**, then **Run anyway**.
3. The installer asks whether you use Adobe apps (you can change it later in Settings). The app
   then finds your installed apps on its own. With **Fewer admin prompts** (on by default) it asks
   for admin rights once, then never again.

Requires Windows 10 or 11 (64-bit).

## Privacy

Everything runs on your PC. There are no accounts, ads or tracking. The app only contacts the
publishers' own download, release and web servers (for versions, change lists and icons),
Microsoft's winget catalog, and its own update feed to check for new versions of itself. That
check sends the app's version and a random install ID (made once, not linked to you) so new
versions can be counted; nothing else.

## Build from source

Requires [Rust](https://rustup.rs) 1.95+ and Node.js 20+.

```bash
npm install
npx tauri build
```

The installer is written to `src-tauri/target/release/bundle/nsis/`. Builds from source use the
official update feed (`src-tauri/.update-feed` and `.update-key`), so they get updates like the
released app. To point a fork at its own feed, set `UPDATER3D_FEED_URL` (and `UPDATER3D_UPDATE_KEY`)
at build time.

<details>
<summary>Development and tests</summary>

```bash
npm run dev                  # interface only, with demo data, at http://localhost:1420
npx tauri dev                # the whole app
npm run check && npm test    # interface checks and tests
cd src-tauri && cargo test   # unit and engine tests
cargo test --test live_probe -- --ignored --nocapture   # live checks against real feeds
```

Demo data states: `?state=ready|updating|current|first|offline|blocked|failed|lowdisk|ccdown|signedout`.

Built with [Tauri 2](https://tauri.app): a Rust backend and a Svelte 5 interface.

</details>

## License

[MIT](LICENSE). Adobe, Creative Cloud, Photoshop and Substance are trademarks of Adobe. Blender is
a trademark of the Blender Foundation. This project is not affiliated with Adobe, the Blender
Foundation, ArtCraft, Epic Games or Microsoft.

Made with AI.
