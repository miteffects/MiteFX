#!/bin/sh
# MiteFX for Linux: copies the plugins where hosts look for them.
#   ./install.sh               for you:        ~/.vst3 and ~/.clap
#   sudo ./install.sh --system for every user: /usr/lib/vst3 and /usr/lib/clap
#   ./install.sh --uninstall   removes them again (add --system for the system copy)
set -e
cd "$(dirname "$0")"
vst3="$HOME/.vst3"
clap="$HOME/.clap"
remove=
for arg in "$@"; do
  case "$arg" in
    --system) vst3=/usr/lib/vst3; clap=/usr/lib/clap ;;
    --uninstall) remove=1 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done
if [ -n "$remove" ]; then
  for b in VST3/*.vst3; do rm -rf "$vst3/$(basename "$b")"; done
  for b in CLAP/*.clap; do rm -f "$clap/$(basename "$b")"; done
  echo "MiteFX removed from $vst3 and $clap."
  exit 0
fi
mkdir -p "$vst3" "$clap"
for b in VST3/*.vst3; do
  rm -rf "$vst3/$(basename "$b")"
  cp -R "$b" "$vst3/"
done
cp CLAP/*.clap "$clap/"
echo "MiteFX installed: VST3 in $vst3, CLAP in $clap. Rescan plugins in your DAW."
