# iUtil

A small Windows-first utility for working with data from a connected iPhone.

The first feature is exporting iMessage conversations from an iOS backup to
JSON, CSV, and HTML, including attachment media.

## Status

Early development. The app is being built feature-first; hardening, tests, and
CI will follow.

## Requirements

- Windows 10/11
- Rust stable
- For USB device access: Apple's Mobile Device Service, installed with
  [iTunes](https://www.apple.com/itunes/download/win64) or the Apple Devices app
- An iPhone that has been unlocked and trusted by this computer

## Build and run

```powershell
cargo run
```

## Privacy and security

iMessage data is highly sensitive. This project:

- never stores backup passwords; they are used in memory for the current export
- keeps temporary backups in a private app-managed folder and removes them after
  export
- does not log message contents by default
- does not include telemetry

Do not commit real backups, exports, or pairing records. The `.gitignore` covers
the common cases, but always double-check `git status` before pushing.

## License

GPL-3.0-or-later. The iMessage parser dependency is GPL-3.0-or-later.
