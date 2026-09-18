#!/bin/sh
# Build the notification helper app bundle (see src-tauri/notifier/main.swift).
#
# Produces src-tauri/notifier/build/Pontifex Notifier.app, which tauri.conf.json
# lists as a bundle resource. Runs before `tauri dev` and `tauri build`; a
# no-op on platforms without swiftc, where the helper is simply absent.
set -e
cd "$(dirname "$0")/../src-tauri/notifier"
if ! command -v swiftc >/dev/null 2>&1; then
  echo "build-notifier: swiftc not found; skipping the notification helper" >&2
  exit 0
fi
# The version stamps the bundle so Pontifex can tell an installed copy is stale.
# A hash of the inputs, so any change to the source or plist is a new version.
VERSION=$( (cat main.swift Info.plist; echo "${APPLE_SIGNING_IDENTITY:-adhoc}") | shasum | cut -c1-12)
APP="build/Pontifex Notifier.app"
if [ -f "$APP/Contents/MacOS/notifier" ] && [ "$(cat build/.version 2>/dev/null)" = "$VERSION" ]; then
  exit 0
fi
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
sed "s/NOTIFIER_VERSION/$VERSION/g" Info.plist > "$APP/Contents/Info.plist"
cp ../icons/icon.icns "$APP/Contents/Resources/icon.icns"
swiftc -O -framework UserNotifications -framework AppKit main.swift -o "$APP/Contents/MacOS/notifier"
# Signed with the same identity Tauri uses for the app when one is configured
# (APPLE_SIGNING_IDENTITY, as in the release workflow), with the hardened
# runtime notarization demands of every nested Mach-O — the bundler does not
# re-sign resources, so a helper signed ad-hoc here would fail notarization.
# Without an identity it is ad-hoc signed, like the app itself.
if [ -n "$APPLE_SIGNING_IDENTITY" ]; then
  codesign --force --options runtime --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$APP"
else
  codesign --force --sign - "$APP" >/dev/null 2>&1
fi
echo "$VERSION" > build/.version
echo "build-notifier: built $APP ($VERSION)"
