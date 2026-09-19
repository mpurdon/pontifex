#!/bin/bash
# Cargo, with the dev binary signed under a stable identity before it runs.
#
# Tauri runs this in place of `cargo` (build.runner in tauri.conf.json), and
# starts the app with `cargo run`. An unsigned dev build is a new application
# to macOS every time it is rebuilt, so the keychain asks permission for the
# Jira and GitHub secrets after every change to the Rust side — and a
# dismissed dialog is remembered as a refusal. Signed with a real identity,
# the binary's designated requirement is the same across builds and the
# keychain asks once.
#
# So `run` becomes: build with the same flags, sign, then exec the binary
# with the app's own arguments. The exec keeps the PID Tauri is watching.
# Everything else is passed to cargo untouched.
#
# The identity is APPLE_SIGNING_IDENTITY when set (the release workflow sets
# it), else the first Developer ID Application certificate in the keychain.
# Without either, or off macOS, this is plain cargo.
set -e
if [ "$1" != "run" ]; then
  exec cargo "$@"
fi
shift

build_args=()
app_args=()
while [ $# -gt 0 ]; do
  if [ "$1" = "--" ]; then
    shift
    app_args=("$@")
    break
  fi
  build_args+=("$1")
  shift
done

cargo build "${build_args[@]}"

profile=debug
for a in "${build_args[@]}"; do
  [ "$a" = "--release" ] && profile=release
done
target="${CARGO_TARGET_DIR:-$(cd "$(dirname "$0")/../src-tauri" && pwd)/target}"
bin="$target/$profile/pontifex"

identity="${APPLE_SIGNING_IDENTITY:-}"
if [ -z "$identity" ] && command -v security >/dev/null 2>&1; then
  identity=$(security find-identity -v -p codesigning 2>/dev/null \
    | grep 'Developer ID Application' | head -1 | awk '{print $2}')
fi
# Only a fresh link leaves an ad-hoc signature; a binary cargo did not
# touch still carries the last one.
if [ -n "$identity" ] && command -v codesign >/dev/null 2>&1 \
  && codesign -dv "$bin" 2>&1 | grep -q '^Signature=adhoc'; then
  codesign --force --sign "$identity" --identifier dev.codenaked.pontifex "$bin" >&2
fi

exec "$bin" "${app_args[@]}"
