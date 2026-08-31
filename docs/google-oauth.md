# Google OAuth setup

The plugin reads Google Calendar events and Google Tasks from multiple Google
accounts. It does not create, edit, or delete cloud data.

## Create a Google OAuth client

1. Create a project in the [Google Cloud Console](https://console.cloud.google.com/).
2. Enable the **Google Calendar API** and **Google Tasks API**.
3. Configure the OAuth consent screen. While the app is in Testing, add every
   Google account you will connect as a test user. [Testing authorizations can
   expire after seven days](https://developers.google.com/identity/protocols/oauth2#expiration),
   so move a personal consent screen to Production for daily use.
4. Create an OAuth client with application type **Desktop app**.
5. Download the client JSON to:

   ```text
   ~/.config/omarchy/calendar/google-client.json
   ```

6. Restrict the file to your user:

   ```bash
   chmod 600 ~/.config/omarchy/calendar/google-client.json
   ```

## Connect accounts

Open the calendar panel and click **+**. Google opens in your default browser.
Repeat for every account you want to combine.

You can also connect an account from a terminal:

```bash
omarchy-calendar add
```

Useful diagnostics:

```bash
omarchy-calendar status
omarchy-calendar refresh
omarchy-calendar list --start 2026-08-31 --end 2026-09-01
```

## Private local data

- Refresh tokens are stored in Secret Service under an opaque account ID.
- Cached metadata is stored at
  `~/.local/state/omarchy/calendar/agenda.db` with mode `0600`.
- The local API uses `$XDG_RUNTIME_DIR/omarchy-calendar.sock` with mode `0600`.
- OAuth tokens are not placed in command arguments, logs, SQLite, QML, or the
  socket protocol.

Removing an account from the panel attempts Google token revocation, removes
its Secret Service item, and deletes its cached rows.
