# gdrive-linux

[![Release](https://img.shields.io/github/v/release/psyycker/google-drive-desktop-linux)](https://github.com/psyycker/google-drive-desktop-linux/releases)
[![CI](https://github.com/psyycker/google-drive-desktop-linux/actions/workflows/ci.yml/badge.svg)](https://github.com/psyycker/google-drive-desktop-linux/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Google Drive for Linux.** Two-way sync, a tray icon, a status window and a CLI, with no
mounts or FUSE. Written in Rust.

A Google Drive for desktop–style client for Linux: it mirrors the **My Drive** folders
you choose (or all of it) into a local folder (`~/GoogleDrive` by default) and keeps both sides in sync —
edits, renames, moves and deletions travel both ways, conflicts never lose data,
and Google Docs/Sheets/Slides appear as `.gdoc`/`.gsheet`/`.gslides` link files
that open in the browser.

## Why this one?

* **Real two-way sync** into a normal folder: any app can use your files, offline included.
* **Only what you need:** pick the Drive folders to download, in the Folders tab or with `gdrive folders add Work/Reports`; the rest stays on Drive.
* **Safe by design:** conflicts keep both versions, deletions go to the trash on both sides, and a missing sync folder never wipes Drive.
* **Native:** a Rust daemon (systemd user service) and a small Tauri tray app; no Electron, no FUSE.
* **Easy to install:** single-file AppImage, `.deb` and `.rpm`, plus lighter terminal-only `.deb`/`.rpm` packages, for x86_64 and aarch64.
* **Split across disks:** keep chosen folders on another drive (e.g. games on an SSD, the rest on an HDD); they stay visible in the Drive folder.
* **File manager menu:** in GNOME Files, right-click to open or copy a file's Drive link, sync now, or move a folder to another drive.
* **Scriptable:** everything the window does is available from the `gdrive` CLI, so it runs fine on a headless machine.

Compared with the alternatives: `rclone` is an excellent general tool but has no tray, UI
or live two-way watcher out of the box; Insync and similar clients are paid and closed
source. This one is free, open source and focused only on Google Drive.

```
┌──────────────┐  JSON over Unix socket   ┌───────────────────────────────────────┐
│  gdrive-app  │ ───────────────────────▶ │ gdrived (systemd user service)        │
│ tray + window│                          │  ├─ inotify watcher ─┐                │
└──────────────┘                          │  ├─ Changes API poll ┼─▶ reconciler ──┼─▶ Drive API
┌──────────────┐                          │  └─ SQLite: remote tree + synced tree │
│ gdrive (CLI) │ ───────────────────────▶ │                                       │
└──────────────┘                          └───────────────────────────────────────┘
```

| Component | What it is |
|---|---|
| `crates/gdrive-core` | Sync engine, Drive API client, OAuth, state DB, IPC protocol |
| `crates/gdrived` | The daemon; runs the engine and serves `$XDG_RUNTIME_DIR/gdrive-linux.sock` |
| `crates/gdrive-cli` | `gdrive` command: status, pause/resume, login, config, `open file.gdoc` |
| `app/src-tauri` | Tauri shell: tray icon (StatusNotifierItem via `ksni`) and the window's commands |
| `app/src` | The status/settings window: React + TypeScript, built with Vite |

## How sync works

The engine keeps three views of every item and compares them:

* **remote tree** — a mirror of Drive metadata, updated from the Drive Changes API every 15 s (configurable);
* **local tree** — the files on disk, watched with inotify (plus an hourly safety rescan);
* **synced tree** — the state both sides last agreed on.

Any change only marks an item *dirty*. The reconciler compares the three versions
to tell which side changed, so replaying an event is harmless and a missed one is
caught by the next scan.

| Situation | Result |
|---|---|
| Edited locally | New revision uploaded to the **same** Drive file (history kept) |
| Edited on Drive | Downloaded atomically (temp file + rename), mtime preserved |
| Edited on both sides | Drive version kept at the original path; yours saved as `name (conflicted copy DATE HOST).ext` and uploaded too |
| Renamed/moved locally | Renamed/moved on Drive (detected by inode — no re-upload) |
| Renamed/moved on Drive | Renamed/moved locally |
| Deleted locally | Moved to the **Drive trash** (recoverable for 30 days) |
| Deleted on Drive | Moved to your **desktop trash** (configurable) |
| Deleted on one side, edited on the other | The edit wins; the file is restored |
| Same content already on both sides | Linked without transferring anything |
| Sync folder missing/unmounted | Sync stops with an error; nothing is deleted remotely |
| Different account signed in | Previous sync state is discarded first |

### Selected folders

A new setup syncs **nothing** until you pick folders; the whole Drive is still read
(names only) so you can browse it:

```sh
gdrive folders browse              # folders at the top of your Drive ([x] synced, [~] partly)
gdrive folders browse Work         # …and inside Work
gdrive folders add Work/Reports Photos
gdrive folders remove Photos       # local copy removed; it stays on Drive
gdrive folders                     # what is synced
gdrive folders all                 # sync the whole Drive instead
gdrive folders only Work --yes     # back from the whole Drive to just some folders
```

A selected folder comes with everything inside it, and the folders on the way to it
are created so it sits at its usual path (`Work/Reports` creates an otherwise empty
`Work`). The choice follows a folder that is renamed or moved, on Drive or locally.
Outside the selected folders nothing is uploaded: a file you create at the top of the
Drive folder, or directly in `Work`, stays on this computer only. Moving a synced file
out of the selected folders moves it on Drive as well, then stops syncing it.

When a folder is unselected, or moved out of the selection on Drive, its local copy is
deleted (Drive keeps it, so it doesn't go to the desktop trash). Files changed here but
not uploaded yet are left in place instead, no longer synced. Configs from before this
feature have no selection and keep syncing the whole Drive. In the window, the
**Folders** tab does the same with a tree of your Drive's folders.

Native Google files (Docs, Sheets, Slides, Forms, Drawings…) become small JSON link
files; double-clicking opens them in the browser. Deleting or renaming a link file
deletes/renames the document, like the official client.

### Folders on other drives

Settings → **Other drives** (or `gdrive config location add SSD /mnt/ssd/GDrive`) adds
a folder on another disk. Moving a Drive folder there (Settings, right-click in Files,
or `gdrive move Gaming SSD`) copies its files over, then leaves a symlink at its old
spot so the whole Drive is still browsable from the main folder. Drive itself never
changes: only where the files sit on disk does. Syncing pauses during a move, and a
disk that isn't mounted stops syncing with an error instead of deleting anything.
Moving a file by hand between a folder on one disk and a folder on another (e.g.
dragging in Files) is a copy across disks, so it is uploaded again as a new file.

Ignored automatically: editor swap/lock files (`.~lock.*#`, `.*.swp`, `*~`, …),
partial downloads (`*.part`, `*.crdownload`) and the client's own `.gdrive-linux/`
folder. Add your own glob patterns in Settings.

## Download

Prebuilt packages for **x86_64** and **aarch64** are attached to every
[GitHub release](https://github.com/psyycker/google-drive-desktop-linux/releases):

Two editions: **desktop** (tray icon, status window, CLI) and **terminal-only** (the
daemon and the `gdrive` CLI, about half the size, with no WebKitGTK/GTK dependency).
They contain the same daemon and CLI, so their packages replace each other.

| File | Edition | For |
|---|---|---|
| `gdrive-linux-<version>-<arch>.AppImage` | desktop | Any distro: `chmod +x` and run. One file holding the app, the daemon (`--daemon`) and the CLI (`--cli …`). |
| `gdrive-linux_<version>_<arch>.deb` | desktop | Debian, Ubuntu, Mint, Pop!_OS |
| `gdrive-linux-<version>-1.<arch>.rpm` | desktop | Fedora, openSUSE, RHEL |
| `gdrive-linux-cli_<version>-1_<arch>.deb` | terminal | Debian, Ubuntu, … (servers included) |
| `gdrive-linux-cli-<version>-1.<arch>.rpm` | terminal | Fedora, openSUSE, RHEL |
| `gdrive-linux-<version>-<arch>-cli.tar.gz` | terminal | Any distro, no package manager: `gdrived` + `gdrive` + the systemd user unit |

Switching editions: `sudo apt install ./gdrive-linux-cli_….deb` replaces the desktop
package by itself; on Fedora use `sudo dnf swap gdrive-linux ./gdrive-linux-cli-….rpm`
(or the other way round). Settings and sync state are kept.

Releases are built by `.github/workflows/release.yml` whenever a `v*` tag is pushed:

```sh
git tag v0.1.0 && git push origin v0.1.0
```

## 1. Create a Google OAuth client (one-time, ~5 minutes)

Google requires every app that talks to Drive to have its own OAuth client:

1. Open <https://console.cloud.google.com/> and create a project (any name).
2. **APIs & Services → Library** → enable **Google Drive API**.
3. **APIs & Services → OAuth consent screen** (Google Auth Platform):
   user type **External**; fill in the app name and your email; add yourself as a test user.
4. **Publish the app** (Audience → *Publish app* → status *In production*).
   **Do this:** in *Testing* status Google expires refresh tokens after 7 days, which
   would sign you out every week. You don't need verification for personal use; you'll
   just see an "unverified app" warning once when signing in (*Advanced → Go to …*).
5. **Credentials → Create credentials → OAuth client ID** → application type
   **Desktop app**. Copy the **Client ID** and **Client secret**.

## 2. Build and install (from source)

On Bazzite/Silverblue, the Tauri app is built inside a distrobox that has the
WebKitGTK headers; the resulting binary runs on the host.

```sh
# one-time: Rust, Node.js 20.19+ (for the window's frontend) and a build box with the Tauri dependencies
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
distrobox create --name gdrive-dev --image registry.fedoraproject.org/fedora-toolbox:44
distrobox enter gdrive-dev -- sudo dnf install -y webkit2gtk4.1-devel gtk3-devel \
    librsvg2-devel openssl-devel libsoup3-devel javascriptcoregtk4.1-devel gcc gcc-c++ pkg-config

scripts/build.sh      # release build (uses the box automatically when needed)
scripts/install.sh    # ~/.local/bin, systemd user service, desktop entries, .gdoc MIME type
```

On a regular distro with `webkit2gtk4.1-devel` installed, `scripts/build.sh` builds
directly. Both build the frontend into `app/dist` first, which the app binary embeds;
a bare `cargo build -p gdrive-app` needs `npm --prefix app ci && npm --prefix app run build` beforehand.
For UI work, `cd app && cargo tauri dev` serves the window from Vite with hot reload
(quit the installed app first: only one instance runs at a time).

The tray icon needs a StatusNotifierItem host; on GNOME that's the
*AppIndicator and KStatusNotifierItem Support* extension (enabled by default on Bazzite/Ubuntu).

## 3. Set up and sign in

Open **Google Drive (gdrive-linux)** from the app grid, paste the client ID and
secret, then click **Sign in**. Or from a terminal:

```sh
gdrive setup
```

`gdrive setup` goes through everything in order, and skips what is already done, so
it is safe to run again:

1. starts the background service, and has it start at login (installing a systemd
   user unit if the package didn't ship one);
2. walks through creating the Google Cloud project and OAuth client of section 1,
   page by page, with links that open in your project; then it finds the
   `client_secret_*.json` you downloaded to `~/Downloads`, or asks for the JSON's
   path or for the ID and secret;
3. asks where the files go (`~/GoogleDrive` by default);
4. prints the sign-in address and what Google will show (including the "unverified
   app" warning). With the browser on another computer, paste back the address of the
   page that fails to load and it finishes the sign-in. If Google refuses, it says
   what to change in the console and tries again;
5. checks that your Drive can actually be read, and if not (the Drive API is off,
   access to files wasn't allowed), says how to fix it and checks again;
6. lists your Drive's folders to choose from (`2/` looks inside folder 2, `..` goes
   back, `all` syncs the whole Drive).

For scripted installs, flags answer the questions:
`gdrive setup --client-json FILE --local-folder ~/Drive --sync Work --sync Photos --yes`
(or `--client-id ID --client-secret SECRET`, `--whole-drive`). Signing in still needs
a browser once.

The individual steps are also commands of their own:

```sh
gdrive config client <CLIENT_ID> <CLIENT_SECRET>
gdrive login          # opens the browser
gdrive folders browse # choose what to download (see "Selected folders")
gdrive folders add Work
```

### Without a desktop (servers, SSH)

Everything works from a terminal. Install the terminal-only package, then run
`gdrive setup` as the user who syncs:

```sh
sudo apt install ./gdrive-linux-cli_<version>-1_amd64.deb      # Debian, Ubuntu
sudo dnf install ./gdrive-linux-cli-<version>-1.x86_64.rpm     # Fedora, RHEL
gdrive setup
```

Over SSH it also offers to keep syncing after you log out (`loginctl enable-linger`).
Without a package manager, use the `-cli.tar.gz` bundle instead:

```sh
install -m755 gdrived gdrive ~/.local/bin/
gdrive setup
```

To build the packages yourself: `cargo build --release -p gdrived -p gdrive-cli`, then
`scripts/cli-packages.sh` (needs [nfpm](https://nfpm.goreleaser.com)).

Google sends the browser back to `http://127.0.0.1:<port>` on the machine running the
daemon, so with the browser on another computer, that last page fails to load. Copy its
address from the address bar and paste it into `gdrive setup`. With `gdrive login`,
finish on the server with `curl '<address>'` instead, or forward the port beforehand
(`ssh -L <port>:127.0.0.1:<port> server`, the port is in the printed address).

## Everyday use

```sh
gdrive status            # state, account, quota, transfers, errors, recent activity
gdrive pause | resume
gdrive folders           # which Drive folders are synced (see "Selected folders")
gdrive sync              # check Drive for changes right now
gdrive resync            # re-read the whole Drive and rescan the folder
gdrive update            # install the newest release (--check to only look)
gdrive config            # show settings
gdrive config folder ~/Drive
gdrive config location add SSD /mnt/ssd/GDrive   # another drive for some folders
gdrive move Gaming SSD   # keep Drive's Gaming folder on it (`main` moves it back)
gdrive open Report.gdoc  # open a Docs link file in the browser
gdrive logout            # forget the account (local files stay)
journalctl --user -u gdrived -f   # daemon logs
```

## Contributing

Issues and pull requests are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md).
If the project is useful to you, a star helps others find it.

## License

[MIT](LICENSE)

## Files

| Path | Contents |
|---|---|
| `~/.config/gdrive-linux/config.toml` | Settings |
| `~/.local/share/gdrive-linux/token.json` | OAuth refresh token (mode 0600) |
| `~/.local/share/gdrive-linux/state.db` | Remote tree + synced tree (SQLite) |
| `<sync folder>/.gdrive-linux/` | Partial downloads; marker that the folder is the synced one (same on each other drive) |
| `~/.local/share/nautilus-python/extensions/gdrive_linux.py` | GNOME Files menu (installed by the daemon when nautilus-python is present) |

## Limitations

* Mirrors **My Drive** only. Shared drives and "Shared with me" items that aren't
  in My Drive are not synced; Drive shortcuts are skipped.
* Mirror mode only: everything in the selected folders is downloaded (no on-demand
  "streaming" filesystem). Selection is by folder; single files can't be picked.
* Remote changes arrive by polling (Drive push notifications need a public HTTPS
  endpoint), so they show up within the poll interval.
* Files that appear in several folders on Drive (legacy multi-parent) are placed under their first parent.
* The refresh token is stored in a user-only file rather than the Secret Service keyring.
