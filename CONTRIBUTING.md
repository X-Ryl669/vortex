# Contributing

## Current status: beta — issues welcome, PRs later

Vortex is in public beta. What helps most right now:

- **Bug reports** — especially device/ROM-specific quirks. BLE behavior
  varies wildly between vendors (MIUI, One UI, ColorOS…), and Linux
  desktops differ too (GNOME/KDE, X11/Wayland, PipeWire versions).
  Please include: phone model + Android/ROM version, Linux distro +
  desktop environment, and what you did / what happened.
- **Feature feedback** — what feels rough, what's missing for your setup.

Open a GitHub issue for either.

## Pull requests

Not yet, please. The architecture is still moving fast and review
bandwidth is limited — a PR written against today's code is likely to be
obsolete before it can be merged. This will open up once the codebase
settles; watch the releases.

Small documentation fixes (typos, broken links) are the exception —
those are always welcome.

## Development model

Day-to-day development happens in a private working repo; this public
repo receives curated release snapshots (the Linux-kernel model). That's
why history here is release-sized commits rather than granular ones.

## Commit style (for when PRs open)

Conventional Commits:

```text
feat(pairing): add SAS confirmation reducer
fix(protocol): reject out-of-order BLE frames
docs(security): clarify key storage model
```

## Security issues

Please do NOT open a public issue for vulnerabilities — use the
maintainer contact on the GitHub profile instead.
