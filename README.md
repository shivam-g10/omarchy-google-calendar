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
- Reuses Omarchy's existing `brave-bin` session through its verified local handoff socket and
  prevents duplicate sign-in windows.
- Uses the desktop's normal URL launcher for Firefox, Chrome, Chromium, and other defaults.
- Opens from cached agenda data immediately; network sync runs in the background or on demand.
- Lets an abandoned Google sign-in be cancelled immediately with the **×** action.
- Marks expired Google grants and reconnects each affected account without deleting cached events.
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

While the Google app remains in Testing, Calendar and Tasks grants expire after
seven days. Select an affected account, then use **Sign in again** in the error
banner.

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

## Agent access and CLI

Agents can use the [calendar skill](skills/omarchy-calendar/SKILL.md) to read the
same cached agenda as the panel. Install that folder into your agent's skill
directory (for local Codex, `~/.codex/skills/omarchy-calendar/`). The backend
installer does not install agent skills automatically.

All commands use the `omarchy-calendar` prefix. No arguments displays help.

| Namespace | Commands |
| --- | --- |
| `account` | `list`, `add [--label LABEL]`, `reconnect ACCOUNT_ID`, `cancel`, `remove ACCOUNT_ID` |
| `event` | `list [--start DATE] [--end DATE] [--account ID]`, `open ITEM_ID` |
| `task` | `list [--start DATE] [--end DATE] [--account ID]`, `open ITEM_ID` |
| `agenda` | `list [--start DATE] [--end DATE] [--account ID]` |
| `service` | `status`, `refresh [--account ID]`, `run` |

`agenda list` returns mixed events and tasks. `event list` and `task list` filter
items by type while retaining context. `account list` returns the accounts array
inside an object. Namespace-specific `open` rejects the other item's ID type.
Namespaces and commands support `--help`.

The former root commands have been replaced: use `account add/remove/...`,
`agenda list`, and `service status/refresh/run`. Update external scripts accordingly.
The installed systemd unit uses `service run`; the QML socket protocol is unchanged.

For a custom range, provide both dates. Start defaults to today and end defaults
independently to tomorrow. Date boundaries use system local time; the end is
exclusive. Whole-day ranges preserve all-day events and dated task context.

```bash
omarchy-calendar agenda list --start 2026-09-14 --end 2026-09-21
```

`agenda list`/`get_agenda` returns `start`, `end`, `items`, `generatedAt`, and additive
`context` metadata. Context contains `configured` and `accounts`, each with `id`,
`label`, `enabled`, `syncing`, `lastSync`, `errorCode`, and `error`. Account filters
scope both items and context; an unknown explicit ID returns `account_not_found`.
All-account context includes disabled accounts, while items exclude them.

`lastSync` means the last successful account sync. `generatedAt` only dates the
response. Null sync timestamps, missing accounts, and errors distinguish limited
cached data from a healthy empty agenda. These are best-effort local observations,
not an atomic remote snapshot or a guarantee of complete cache coverage.

Reads do not trigger synchronization, access credentials, or wait for a network
refresh. Background polling starts after two seconds and waits 270–330 seconds
after each refresh attempt. No routine `status` preflight is necessary.

Explicit `service refresh` usually returns after its attempt: inspect per-account results
even when the command succeeds. `started: false` with `already_syncing` means no
new refresh started. Omit `--account` for all accounts rather than passing `all`.

Events come from selected calendars; tasks are incomplete and date-based, excluding
undated tasks. Agent reasoning and answer formatting are outside this interface.
Command errors return JSON (`ok: false`, `error.code`, `error.message`) and exit 1;
successful commands return the JSON result directly, except textual help.

If an agent's sandbox blocks the private Unix socket, reads return
`daemon_access_denied`. This does not mean the daemon stopped. Use the agent host's
normal approval mechanism for local socket access when available; do not loosen
socket permissions or restart the daemon to work around sandbox restrictions.

## Development checks

Validate the Omarchy plugin and run the backend checks:

```bash
omarchy plugin validate .
cargo fmt --manifest-path calendar-rs/Cargo.toml --check
cargo test --locked --all-targets --manifest-path calendar-rs/Cargo.toml
cargo clippy --locked --all-targets --manifest-path calendar-rs/Cargo.toml -- -D warnings
```

The local backend protocol is newline-delimited JSON over a private Unix
socket. Its supported methods are `get_state`, `get_agenda`, `refresh`,
`add_account`, `reconnect_account`, `cancel_add_account`, `remove_account`, and `open_item`.

## License

[MIT](LICENSE). Portions derived from Omarchy retain their original notice in
[THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES).
