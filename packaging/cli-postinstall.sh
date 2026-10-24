#!/bin/sh
# After a first install (not an upgrade): the service runs per user, so say how to set it up.
# deb passes "configure" with no previous version; rpm passes the install count, 1.
if { [ "$1" = configure ] && [ -z "$2" ]; } || [ "$1" = 1 ]; then
  cat <<'MSG'
gdrive-linux-cli is installed. As the user who will sync, run:
  gdrive setup
It starts the service, then walks through the Google client, sign-in and folders.
MSG
fi
exit 0
