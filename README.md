# Omarchy Google Calendar

A native, multi-account Google Calendar and Tasks panel for Omarchy. The bar
clock shows the weekday and date — for example, `Monday, 31st Aug, 21:45` — and
opens a unified calendar when clicked.

![Calendar panel with multi-account filter and add-account button](docs/calendar-panel.png)

## Features

- Combines events and date-based tasks from multiple Google accounts.
- Filters the agenda by account while keeping one calendar view.
- Shows calendar dots, week numbers, year progress, and a daily agenda.
- Connects accounts through browser-based OAuth with PKCE and state validation.
- Stores refresh tokens in Secret Service, not in the plugin or SQLite cache.
- Uses a native Rust service with incremental calendar sync and expired
  sync-token recovery.
- Keeps the bar label configurable through Omarchy's bar settings.

The integration is read-only. It never creates, edits, or deletes Google
Calendar events or Google Tasks.

## Install

Requirements:

- Omarchy with plugin support
- current stable Rust and Cargo with edition 2024 support
- SQLite and `pkg-config`
- `secret-tool` from libsecret
- `xdg-open` from xdg-utils and a default browser
- a user systemd session

Install and enable the plugin:

```bash
omarchy plugin add https://github.com/shivam-g10/omarchy-google-calendar.git --enable
~/.config/omarchy/plugins/shivam.clock/scripts/install-backend
```

Then follow [Google OAuth setup](docs/google-oauth.md) and click **+** in the
calendar panel to connect each account.

## Update

```bash
omarchy plugin update shivam.clock
~/.config/omarchy/plugins/shivam.clock/scripts/install-backend
```

The second command rebuilds the Rust backend and restarts its user service.

## Remove

Remove connected accounts from the panel first so the plugin can revoke their
Google tokens. Then remove the backend and plugin:

```bash
~/.config/omarchy/plugins/shivam.clock/scripts/uninstall-backend
omarchy plugin remove shivam.clock --yes
```

The backend uninstaller deliberately retains local account secrets and cached
calendar data. Removing accounts from the panel clears their credentials and
cache rows.

## Clock format

Set the bar widget's format to:

```text
dddd, {ordinal} MMM, HH:mm
```

This renders values such as `Monday, 31st Aug, 21:45`.

## Development

Validate the Omarchy plugin and run the backend checks:

```bash
omarchy plugin validate .
cargo fmt --manifest-path calendar-rs/Cargo.toml --check
cargo test --locked --all-targets --manifest-path calendar-rs/Cargo.toml
cargo clippy --locked --all-targets --manifest-path calendar-rs/Cargo.toml -- -D warnings
```

The local backend protocol is newline-delimited JSON over a private Unix
socket. Its supported methods are `get_state`, `get_agenda`, `refresh`,
`add_account`, `remove_account`, and `open_item`.

## License

[MIT](LICENSE). Portions derived from Omarchy retain their original notice in
[THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES).
