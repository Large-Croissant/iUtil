# iUtil

A small Windows-first utility for working with data from an iPhone.

The first feature is exporting iMessage conversations to JSON, CSV, and HTML,
including original attachment media. You can export from an existing iOS backup
folder or make a fresh backup directly from a USB-connected iPhone.

## Status

Early development, but the iMessage export works end to end. Hardening, tests,
and CI will follow.

## Requirements

- Windows 10/11
- Rust stable
- For USB device access: Apple's Mobile Device Service, installed with
  [iTunes](https://www.apple.com/itunes/download/win64) or the Apple Devices app.
  The classic iTunes download is the most reliable option for USB discovery.
- An iPhone that has been unlocked and trusted by this computer

The iPhone must be unlocked at least once after it starts up. If it has never
been paired with this PC, iUtil asks the iPhone to show a **Trust This Computer**
prompt the first time you export over USB.

## Build and run

```powershell
cargo run
```

For a smaller release build:

```powershell
cargo build --release
```

## Using it

1. Choose **Existing iOS backup folder** and select a backup, or choose
   **Connected iPhone over USB**, plug in the iPhone, and press **Find iPhones**.
2. If the backup is encrypted, enter its password. Passwords are used only in
   memory for this export and are never saved.
3. Pick an output folder and the formats you want.
4. Press **Export messages**.

A USB export makes a full temporary backup first, reads the Messages database
from it, and deletes the temporary backup when the export finishes.

## What gets exported

- `messages.json` - the complete structured export
- `messages.csv`, `chats.csv`, `participants.csv`, `attachments.csv`,
  `reactions.csv`, `edits.csv`, `edit_events.csv`, `components.csv`
- `index.html` - a readable, self-contained conversation archive
- `attachments/` - original attachment files, copied without conversion

## Privacy and security

iMessage data is highly sensitive. This project:

- never stores backup passwords; they are used in memory for the current export
- keeps temporary USB backups in the system temporary folder and removes them
  after the export
- does not log message contents by default
- does not include telemetry or network access beyond talking to the local
  Apple device service

The exported folder is **not encrypted**. Store it somewhere you trust, and
delete it when you no longer need it.

Do not commit real backups, exports, or pairing records. The `.gitignore` covers
the common cases, but always double-check `git status` before pushing.

## License

GPL-3.0-or-later. The iMessage parser dependency is GPL-3.0-or-later.
