# Vortex Linux packaging

The Tauri bundle (`cd ui-tauri && npx tauri build`) produces a `.deb` /
`AppImage` that installs the `vortex-ui-tauri` binary and a desktop entry
for the app menu. What it does NOT do is start Vortex with the session —
the app is a tray app that owns the BLE/LAN link, so it should.

## Autostart (recommended)

```sh
mkdir -p ~/.config/autostart
cp vortex.desktop ~/.config/autostart/
```

## systemd user unit (alternative, headless/WM setups)

```sh
mkdir -p ~/.config/systemd/user
cp vortex.service ~/.config/systemd/user/
systemctl --user enable --now vortex.service
```

Pick one — running both starts two instances.

## Android release signing

`a3/` release builds are signed from `a3/keystore.properties` (gitignored).
Create a keystore once and point the properties file at it:

```sh
keytool -genkeypair -v -keystore ~/keys/vortex.keystore \
  -alias vortex -keyalg RSA -keysize 4096 -validity 10000

cat > a3/keystore.properties <<EOF
storeFile=/home/<you>/keys/vortex.keystore
storePassword=…
keyAlias=vortex
keyPassword=…
EOF
```

Without the file `assembleRelease` still works and emits an unsigned APK.

> ⚠️ The release APK is minified (R8). Before trusting any release build,
> run a full on-device smoke test: pairing, reconnect, call mirror,
> notifications, SMS send/receive, earbuds switch.
