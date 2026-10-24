#!/bin/sh
# After a first install (not an upgrade): the service runs per user, so say how to start it.
# deb passes "configure" with no previous version; rpm passes the install count, 1.
if { [ "$1" = configure ] && [ -z "$2" ]; } || [ "$1" = 1 ]; then
  cat <<'MSG'
gdrive-linux-cli is installed. As the user who will sync, run:
  systemctl --user enable --now gdrived
  gdrive config client <CLIENT_ID> <CLIENT_SECRET>
  gdrive login
On a server, also run `sudo loginctl enable-linger <user>` to keep syncing while logged out.
MSG
fi
exit 0
