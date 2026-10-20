"""GNOME Files (Nautilus) menu items for gdrive-linux.

Adds "Open in Google Drive", "Copy Drive link", "Share…" and "Sync now" to the
right-click menu of items inside the Drive folder. It asks the gdrived daemon over
its Unix socket which Drive file a path is, so nothing shows outside the folder or
while the daemon isn't running.

Installed by gdrived into ~/.local/share/nautilus-python/extensions; it is
overwritten on every daemon start, so edit packaging/nautilus/gdrive_linux.py in
the gdrive-linux repository instead.
"""

import json
import os
import socket

import gi

gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, Gio, GObject, Nautilus  # noqa: E402

# Nautilus waits for the menu while we ask the daemon: never make it wait long.
TIMEOUT = 0.5
# Lookups beyond this many selected items are skipped (one socket round trip each).
MAX_ITEMS = 50


def socket_path():
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if runtime:
        return os.path.join(runtime, "gdrive-linux.sock")
    return "/tmp/gdrive-linux-{}.sock".format(os.environ.get("UID", "user"))


def request(cmd, **args):
    """Sends one request to gdrived and returns its reply, or None if it can't be reached."""
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(TIMEOUT)
            s.connect(socket_path())
            s.sendall((json.dumps(dict(cmd=cmd, **args)) + "\n").encode())
            reply = b""
            while not reply.endswith(b"\n"):
                chunk = s.recv(65536)
                if not chunk:
                    break
                reply += chunk
        return json.loads(reply)
    except (OSError, ValueError):
        return None


def lookup(path):
    """The Drive item synced at `path`, or None if it isn't one."""
    reply = request("lookup", path=path)
    if not reply or reply.get("type") != "item":
        return None
    return reply


def local_path(file_info):
    location = file_info.get_location()
    return location.get_path() if location is not None else None


def open_uri(uri):
    Gio.AppInfo.launch_default_for_uri(uri, None)


class GDriveMenu(GObject.GObject, Nautilus.MenuProvider):
    def get_file_items(self, files):
        if not files or len(files) > MAX_ITEMS:
            return []
        items = []
        for f in files:
            path = local_path(f)
            item = lookup(path) if path else None
            if item is None:
                return []
            items.append(item)
        links = [i["link"] for i in items]

        def open_all(_menu):
            for link in links:
                open_uri(link)

        def copy(_menu):
            Gdk.Display.get_default().get_clipboard().set("\n".join(links))

        def sync(_menu):
            request("sync_now")

        entries = [
            ("open", "Open in Google Drive", open_all),
            ("copy", "Copy Drive link" if len(links) == 1 else "Copy Drive links", copy),
            ("share", "Share…", open_all),
            ("sync", "Sync now", sync),
        ]
        menu = []
        for key, label, action in entries:
            entry = Nautilus.MenuItem(name="GDriveLinux::" + key, label=label)
            entry.connect("activate", action)
            menu.append(entry)
        return menu
