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
2. If the backup is encrypted, enter its password. The same applies when the
   iPhone has encrypted backups turned on. Passwords are used only in memory for
   this export and are never saved.
3. Pick an output folder and the formats you want.
4. Press **Export messages**.

A USB export makes a full temporary backup first, reads the Messages database
from it, and deletes the temporary backup when the export finishes. iOS decides
whether that backup is encrypted. If it is, iUtil needs the iPhone's backup
password to read it. The password is never sent to the phone, and iUtil never
turns encryption on, turns it off, or changes the password.

## What gets exported

- `messages.json` - the complete structured export
- `messages.csv`, `chats.csv`, `participants.csv`, `attachments.csv`,
  `reactions.csv`, `edits.csv`, `edit_events.csv`, `components.csv`
- `index.html` - a readable, self-contained conversation archive
- `attachments/` - original attachment files, copied without conversion

## Troubleshooting

- **"The iPhone rejected this computer's pairing"**: the pairing record stored
  on the PC is stale. Unlock the iPhone, keep it connected, and export again.
  iUtil asks the iPhone to trust the computer again; tap **Trust** and enter the
  passcode if asked.
- **"Encrypted backups are turned on..."**: enter the iPhone's backup password.
  This is the password set in iTunes/Finder or the Apple Devices app, not the
  device passcode.
- **The iPhone is not found**: install iTunes or the Apple Devices app so
  Apple's Mobile Device Service is running, then reconnect the iPhone.

## Privacy and security

iMessage data is highly sensitive. This project:

- never stores backup passwords; they are used in memory for the current export
- never changes your iPhone's backup encryption settings
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
