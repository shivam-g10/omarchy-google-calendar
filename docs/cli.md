# Agent access and CLI

The backend installer installs the [calendar skill](../skills/omarchy-calendar/SKILL.md)
into `${CODEX_HOME:-~/.codex}/skills/omarchy-calendar/`. Updates replace `SKILL.md`;
uninstall removes that file and its directory if empty, preserving user-added files.
Use the same `CODEX_HOME` when installing and uninstalling.

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
