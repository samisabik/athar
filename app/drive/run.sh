#!/bin/sh
# Runs one of this folder's scripts in a test window of the unbundled app, titled "Athar DEBUG".
#   app/drive/run.sh <script> <home> [seconds]
# <script> is a file here, without .js. <home> is the library folder the window uses (ATHAR_HOME);
# name a scratch one, since the scripts add projects and save files. The script's log() lines
# print as "[page] ..." and it ends with "[page] DONE"; the window closes after [seconds] (90).
# Build the app first: cargo build --release, in app/.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
[ $# -ge 2 ] || { sed -n '2,7p' "$0" >&2; exit 2; }
script="$here/$1.js"
bin="$here/../target/release/athar-app"
[ -f "$script" ] || { echo "No script $script" >&2; exit 2; }
[ -x "$bin" ] || { echo "Build the app first: cargo build --release, in app/" >&2; exit 2; }
ATHAR_HOME=$2 ATHAR_DRIVE=$script exec perl -e 'alarm shift; exec @ARGV' "${3:-90}" "$bin"
