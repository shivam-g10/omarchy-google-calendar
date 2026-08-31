use chrono::{Duration, Local};
use omarchy_calendar::{CalendarError, default_paths, run_daemon, send_request};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::env;

struct ParsedTail {
    options: HashMap<String, String>,
    positional: Vec<String>,
}

fn parse_tail(arguments: &[String], allowed_options: &[&str]) -> Result<ParsedTail, CalendarError> {
    let mut options = HashMap::new();
    let mut positional = Vec::new();
    let mut index = 1;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument.starts_with('-') {
            if !allowed_options.contains(&argument.as_str()) {
                return Err(CalendarError::new(
                    "invalid_command",
                    format!("Unrecognized argument: {argument}"),
                ));
            }
            let value = arguments.get(index + 1).ok_or_else(|| {
                CalendarError::new("invalid_command", format!("{argument} requires a value"))
            })?;
            if value.starts_with('-') {
                return Err(CalendarError::new(
                    "invalid_command",
                    format!("{argument} requires a value"),
                ));
            }
            options.insert(argument.clone(), value.clone());
            index += 2;
        } else {
            positional.push(argument.clone());
            index += 1;
        }
    }
    Ok(ParsedTail {
        options,
        positional,
    })
}

fn require_positionals(
    parsed: &ParsedTail,
    expected: usize,
    usage: &str,
) -> Result<(), CalendarError> {
    if parsed.positional.len() != expected {
        return Err(CalendarError::new(
            "invalid_command",
            format!("Usage: {usage}"),
        ));
    }
    Ok(())
}

fn help(command: Option<&str>) {
    let detail = match command {
        Some("daemon") => "daemon",
        Some("status") => "status",
        Some("list") => "list [--start DATE] [--end DATE] [--account ID]",
        Some("add") => "add [--label LABEL]",
        Some("remove") => "remove ACCOUNT_ID",
        Some("refresh") => "refresh [--account ID]",
        Some("open") => "open ITEM_ID",
        _ => {
            println!(
                "Omarchy calendar backend\n\nCommands:\n  daemon\n  status\n  list [--start DATE] [--end DATE] [--account ID]\n  add [--label LABEL]\n  remove ACCOUNT_ID\n  refresh [--account ID]\n  open ITEM_ID"
            );
            return;
        }
    };
    println!("Usage: omarchy-calendar {detail}");
}

fn command() -> Result<Value, CalendarError> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let command = arguments.first().map(String::as_str).unwrap_or("daemon");
    if matches!(command, "help" | "--help" | "-h") {
        help(None);
        return Ok(Value::Null);
    }
    if arguments
        .iter()
        .skip(1)
        .any(|argument| matches!(argument.as_str(), "--help" | "-h"))
    {
        help(Some(command));
        return Ok(Value::Null);
    }
    if command == "daemon" {
        require_positionals(&parse_tail(&arguments, &[])?, 0, "omarchy-calendar daemon")?;
        let exit_code = run_daemon()?;
        std::process::exit(exit_code);
    }
    let (_, _, socket_path) = default_paths()?;
    match command {
        "status" => {
            require_positionals(&parse_tail(&arguments, &[])?, 0, "omarchy-calendar status")?;
            send_request(&socket_path, "get_state", json!({}))
        }
        "list" => {
            let parsed = parse_tail(&arguments, &["--start", "--end", "--account"])?;
            require_positionals(&parsed, 0, "omarchy-calendar list [options]")?;
            let today = Local::now().date_naive();
            let start = parsed
                .options
                .get("--start")
                .cloned()
                .unwrap_or_else(|| today.to_string());
            let end = parsed
                .options
                .get("--end")
                .cloned()
                .unwrap_or_else(|| (today + Duration::days(1)).to_string());
            let account = parsed
                .options
                .get("--account")
                .cloned()
                .unwrap_or_else(|| "all".into());
            send_request(
                &socket_path,
                "get_agenda",
                json!({"start": start, "end": end, "account": account}),
            )
        }
        "add" => {
            let parsed = parse_tail(&arguments, &["--label"])?;
            require_positionals(&parsed, 0, "omarchy-calendar add [--label LABEL]")?;
            let mut parameters = Map::new();
            if let Some(label) = parsed.options.get("--label") {
                parameters.insert("label".into(), Value::String(label.clone()));
            }
            send_request(&socket_path, "add_account", Value::Object(parameters))
        }
        "remove" => {
            let parsed = parse_tail(&arguments, &[])?;
            require_positionals(&parsed, 1, "omarchy-calendar remove ACCOUNT_ID")?;
            send_request(
                &socket_path,
                "remove_account",
                json!({"accountId": parsed.positional[0]}),
            )
        }
        "refresh" => {
            let parsed = parse_tail(&arguments, &["--account"])?;
            require_positionals(&parsed, 0, "omarchy-calendar refresh [--account ID]")?;
            let mut parameters = Map::new();
            if let Some(account) = parsed.options.get("--account") {
                parameters.insert("accountId".into(), Value::String(account.clone()));
            }
            send_request(&socket_path, "refresh", Value::Object(parameters))
        }
        "open" => {
            let parsed = parse_tail(&arguments, &[])?;
            require_positionals(&parsed, 1, "omarchy-calendar open ITEM_ID")?;
            send_request(
                &socket_path,
                "open_item",
                json!({"itemId": parsed.positional[0]}),
            )
        }
        other => Err(CalendarError::new(
            "invalid_command",
            format!("Unknown command: {other}"),
        )),
    }
}

fn main() {
    match command() {
        Ok(Value::Null) => {}
        Ok(value) => println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("serialize result")
        ),
        Err(error) => {
            println!(
                "{}",
                serde_json::to_string(&json!({
                    "ok": false,
                    "error": {"code": error.code, "message": error.message}
                }))
                .expect("serialize error")
            );
            std::process::exit(1);
        }
    }
}
