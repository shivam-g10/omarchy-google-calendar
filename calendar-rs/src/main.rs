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

const COMMANDS: &[(&str, &str, &str)] = &[
    ("account list", "accounts", ""),
    ("account add", "add", "[--label LABEL]"),
    ("account reconnect", "reconnect", "ACCOUNT_ID"),
    ("account cancel", "cancel", ""),
    ("account remove", "remove", "ACCOUNT_ID"),
    (
        "event list",
        "events",
        "[--start DATE] [--end DATE] [--account ID]",
    ),
    ("event open", "event-open", "ITEM_ID"),
    (
        "task list",
        "tasks",
        "[--start DATE] [--end DATE] [--account ID]",
    ),
    ("task open", "task-open", "ITEM_ID"),
    (
        "agenda list",
        "list",
        "[--start DATE] [--end DATE] [--account ID]",
    ),
    ("service status", "status", ""),
    ("service refresh", "refresh", "[--account ID]"),
    ("service run", "daemon", ""),
];

fn help(prefix: &str) {
    println!("Omarchy calendar backend\n\nCommands:");
    for (name, _, options) in COMMANDS {
        if prefix.is_empty() || *name == prefix || name.starts_with(&format!("{prefix} ")) {
            println!("  {name} {options}");
        }
    }
}

fn route(arguments: &[String]) -> Result<Vec<String>, CalendarError> {
    let name = arguments
        .iter()
        .take(2)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let (_, internal, _) = COMMANDS
        .iter()
        .find(|(public, _, _)| *public == name)
        .ok_or_else(|| {
            CalendarError::new(
                "invalid_command",
                format!("Unknown command: {name}. Use omarchy-calendar --help"),
            )
        })?;
    let mut result = vec![internal.to_string()];
    result.extend_from_slice(&arguments[2..]);
    Ok(result)
}

fn command() -> Result<Value, CalendarError> {
    let raw: Vec<String> = env::args().skip(1).collect();
    if raw.is_empty() || matches!(raw[0].as_str(), "help" | "--help" | "-h") {
        help("");
        return Ok(Value::Null);
    }
    if raw.len() == 1
        && COMMANDS
            .iter()
            .any(|(name, _, _)| name.starts_with(&format!("{} ", raw[0])))
    {
        help(&raw[0]);
        return Ok(Value::Null);
    }
    if raw.len() == 2
        && matches!(raw[1].as_str(), "--help" | "-h")
        && COMMANDS
            .iter()
            .any(|(name, _, _)| name.starts_with(&format!("{} ", raw[0])))
    {
        help(&raw[0]);
        return Ok(Value::Null);
    }
    let arguments = route(&raw)?;
    let command = arguments[0].as_str();
    if arguments
        .iter()
        .skip(1)
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        help(&raw[..2].join(" "));
        return Ok(Value::Null);
    }
    let public = raw[..2].join(" ");
    let options = COMMANDS
        .iter()
        .find(|(name, _, _)| *name == public)
        .unwrap()
        .2;
    let usage = format!("omarchy-calendar {public} {options}");
    if command == "daemon" {
        require_positionals(&parse_tail(&arguments, &[])?, 0, &usage)?;
        std::process::exit(run_daemon()?);
    }
    let request = |method: &str, params: Value| {
        let (_, _, socket_path) = default_paths()?;
        send_request(&socket_path, method, params)
    };
    match command {
        "status" | "accounts" => {
            require_positionals(&parse_tail(&arguments, &[])?, 0, &usage)?;
            let state = request("get_state", json!({}))?;
            if command == "accounts" {
                Ok(json!({"accounts": state["accounts"]}))
            } else {
                Ok(state)
            }
        }
        "list" | "events" | "tasks" => {
            let parsed = parse_tail(&arguments, &["--start", "--end", "--account"])?;
            require_positionals(&parsed, 0, &usage)?;
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
            let mut result = request(
                "get_agenda",
                json!({"start": start, "end": end, "account": account}),
            )?;
            filter_items(&mut result, command);
            Ok(result)
        }
        "add" => {
            let parsed = parse_tail(&arguments, &["--label"])?;
            require_positionals(&parsed, 0, &usage)?;
            let mut parameters = Map::new();
            if let Some(label) = parsed.options.get("--label") {
                parameters.insert("label".into(), Value::String(label.clone()));
            }
            request("add_account", Value::Object(parameters))
        }
        "cancel" => {
            require_positionals(&parse_tail(&arguments, &[])?, 0, &usage)?;
            request("cancel_add_account", json!({}))
        }
        "reconnect" => {
            let parsed = parse_tail(&arguments, &[])?;
            require_positionals(&parsed, 1, &usage)?;
            request(
                "reconnect_account",
                json!({"accountId": parsed.positional[0]}),
            )
        }
        "remove" => {
            let parsed = parse_tail(&arguments, &[])?;
            require_positionals(&parsed, 1, &usage)?;
            request("remove_account", json!({"accountId": parsed.positional[0]}))
        }
        "refresh" => {
            let parsed = parse_tail(&arguments, &["--account"])?;
            require_positionals(&parsed, 0, &usage)?;
            let mut parameters = Map::new();
            if let Some(account) = parsed.options.get("--account") {
                parameters.insert("accountId".into(), Value::String(account.clone()));
            }
            request("refresh", Value::Object(parameters))
        }
        "event-open" | "task-open" => {
            let parsed = parse_tail(&arguments, &[])?;
            require_positionals(&parsed, 1, &usage)?;
            let expected = if command == "event-open" {
                "event:"
            } else {
                "task:"
            };
            if !parsed.positional[0].starts_with(expected) {
                return Err(CalendarError::new(
                    "invalid_params",
                    format!("Expected {expected} item ID"),
                ));
            }
            request("open_item", json!({"itemId": parsed.positional[0]}))
        }
        other => Err(CalendarError::new(
            "invalid_command",
            format!("Unknown command: {other}"),
        )),
    }
}

fn filter_items(result: &mut Value, command: &str) {
    let kind = match command {
        "events" => "event",
        "tasks" => "task",
        _ => return,
    };
    if let Some(items) = result["items"].as_array_mut() {
        items.retain(|item| item["type"] == kind);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_route_without_crossing_resource_boundaries() {
        for (public, internal, _) in COMMANDS {
            let args = public
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            assert_eq!(route(&args).unwrap(), vec![internal.to_string()]);
        }
        for invalid in [
            "add",
            "remove",
            "list",
            "event add",
            "task remove",
            "agenda open",
        ] {
            let args = invalid
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            assert_eq!(route(&args).unwrap_err().code, "invalid_command");
        }
        let args = ["account", "remove", "account-1"].map(str::to_owned);
        assert_eq!(route(&args).unwrap(), vec!["remove", "account-1"]);
    }

    #[test]
    fn typed_lists_preserve_context_and_source_records() {
        let mixed = json!({"items": [{"type":"event","id":"event:1"},{"type":"task","id":"task:2"}],"context":{"accounts":[]},"start":"2026-09-14"});
        for (command, kind) in [("events", "event"), ("tasks", "task")] {
            let mut result = mixed.clone();
            filter_items(&mut result, command);
            assert_eq!(result["items"].as_array().unwrap().len(), 1);
            assert_eq!(result["items"][0]["type"], kind);
            assert_eq!(result["context"], mixed["context"]);
            assert_eq!(result["start"], mixed["start"]);
        }
        let mut result = mixed.clone();
        filter_items(&mut result, "list");
        assert_eq!(result, mixed);
    }
}
