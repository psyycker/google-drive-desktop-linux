# gdrive-linux

A Google Drive for desktop–style client for Linux: it mirrors your whole **My Drive**
into a local folder (`~/GoogleDrive` by default) and keeps both sides in sync —
edits, renames, moves and deletions travel both ways, conflicts never lose data,
and Google Docs/Sheets/Slides appear as `.gdoc`/`.gsheet`/`.gslides` link files
that open in the browser.

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

Native Google files (Docs, Sheets, Slides, Forms, Drawings…) become small JSON link
files; double-clicking opens them in the browser. Deleting or renaming a link file
deletes/renames the document, like the official client.

Ignored automatically: editor swap/lock files (`.~lock.*#`, `.*.swp`, `*~`, …),
partial downloads (`*.part`, `*.crdownload`) and the client's own `.gdrive-linux/`
folder. Add your own glob patterns in Settings.

## Download

Prebuilt packages for **x86_64** and **aarch64** are attached to every
[GitHub release](https://github.com/psyycker/google-drive-desktop-linux/releases):

| File | For |
|---|---|
| `gdrive-linux-<version>-<arch>.AppImage` | Any distro: `chmod +x` and run. One file holding the app, the daemon (`--daemon`) and the CLI (`--cli …`). |
| `.deb` | Debian, Ubuntu, Mint, Pop!_OS |
| `.rpm` | Fedora, openSUSE, RHEL |
| `gdrive-linux-<version>-<arch>-cli.tar.gz` | Headless/servers: `gdrived` + `gdrive` + the systemd user unit |

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
gdrive config client <CLIENT_ID> <CLIENT_SECRET>
gdrive login          # opens the browser
gdrive status         # follow the first sync
```

## Everyday use

```sh
gdrive status            # state, account, quota, transfers, errors, recent activity
gdrive pause | resume
gdrive sync              # check Drive for changes right now
gdrive resync            # re-read the whole Drive and rescan the folder
gdrive config            # show settings
gdrive config folder ~/Drive
gdrive open Report.gdoc  # open a Docs link file in the browser
gdrive logout            # forget the account (local files stay)
journalctl --user -u gdrived -f   # daemon logs
```

## Files

| Path | Contents |
|---|---|
| `~/.config/gdrive-linux/config.toml` | Settings |
| `~/.local/share/gdrive-linux/token.json` | OAuth refresh token (mode 0600) |
| `~/.local/share/gdrive-linux/state.db` | Remote tree + synced tree (SQLite) |
| `<sync folder>/.gdrive-linux/` | Partial downloads; marker that the folder is the synced one |

## Limitations

* Mirrors **My Drive** only. Shared drives and "Shared with me" items that aren't
  in My Drive are not synced; Drive shortcuts are skipped.
* Mirror mode only: everything is downloaded (no on-demand "streaming" filesystem).
* Remote changes arrive by polling (Drive push notifications need a public HTTPS
  endpoint), so they show up within the poll interval.
* Files that appear in several folders on Drive (legacy multi-parent) are placed under their first parent.
* Interrupted uploads restart from the beginning rather than resuming mid-file.
* The refresh token is stored in a user-only file rather than the Secret Service keyring.
