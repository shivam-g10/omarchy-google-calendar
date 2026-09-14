use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

// Exercise the actual executable against an isolated socket, never live accounts.
fn exchange(args: &[&str], response: Value) -> (Value, Value) {
    let temp = tempfile::tempdir().unwrap();
    let listener = UnixListener::bind(temp.path().join("omarchy-calendar.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_omarchy-calendar"))
        .args(args)
        .env("XDG_RUNTIME_DIR", temp.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let output = child.wait_with_output().unwrap();
                    panic!(
                        "CLI did not connect: {}",
                        String::from_utf8_lossy(&output.stdout)
                    );
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("accept: {error}"),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut line = String::new();
    BufReader::new(stream.try_clone().unwrap())
        .read_line(&mut line)
        .unwrap();
    let request: Value = serde_json::from_str(&line).unwrap();
    writeln!(
        stream,
        "{}",
        json!({"id":request["id"],"ok":true,"result":response})
    )
    .unwrap();
    drop(stream);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    (request, serde_json::from_slice(&output.stdout).unwrap())
}

#[test]
fn account_and_service_commands_send_scoped_requests() {
    for (args, method, params) in [
        (
            vec!["account", "add", "--label", "Work account"],
            "add_account",
            json!({"label":"Work account"}),
        ),
        (
            vec!["account", "remove", "a1"],
            "remove_account",
            json!({"accountId":"a1"}),
        ),
        (
            vec!["account", "reconnect", "a1"],
            "reconnect_account",
            json!({"accountId":"a1"}),
        ),
        (vec!["account", "cancel"], "cancel_add_account", json!({})),
        (
            vec!["service", "refresh", "--account", "a1"],
            "refresh",
            json!({"accountId":"a1"}),
        ),
        (vec!["service", "refresh"], "refresh", json!({})),
        (
            vec!["event", "open", "event:123"],
            "open_item",
            json!({"itemId":"event:123"}),
        ),
        (
            vec!["task", "open", "task:123"],
            "open_item",
            json!({"itemId":"task:123"}),
        ),
    ] {
        let (request, _) = exchange(&args, json!({"done":true}));
        assert_eq!(request["method"], method);
        assert_eq!(request["params"], params);
    }
    let state = json!({"accounts":[{"id":"a1"}],"status":"idle"});
    let (request, accounts) = exchange(&["account", "list"], state.clone());
    assert_eq!(request["method"], "get_state");
    assert_eq!(accounts, json!({"accounts":state["accounts"]}));
    let (_, status) = exchange(&["service", "status"], state.clone());
    assert_eq!(status, state);
}

#[test]
fn resource_lists_keep_context_and_forward_date_and_account_options() {
    let mixed = json!({"items":[{"type":"event"},{"type":"task"}],"context":{"accounts":[{"id":"a1"}]},"start":"2026-09-14","end":"2026-09-15"});
    for group in ["agenda", "event", "task"] {
        let (request, result) = exchange(
            &[
                group,
                "list",
                "--start",
                "2026-09-14",
                "--end",
                "2026-09-15",
                "--account",
                "a1",
            ],
            mixed.clone(),
        );
        assert_eq!(request["method"], "get_agenda");
        assert_eq!(
            request["params"],
            json!({"start":"2026-09-14","end":"2026-09-15","account":"a1"})
        );
        assert_eq!(result["context"], mixed["context"]);
        if group == "agenda" {
            assert_eq!(result, mixed);
        } else {
            assert_eq!(result["items"], json!([{"type":group}]));
        }
    }
}

#[test]
fn invalid_arguments_are_reported_before_runtime_access() {
    for (args, expected) in [
        (vec!["task", "list", "extra"], "omarchy-calendar task list"),
        (
            vec!["account", "list", "extra"],
            "omarchy-calendar account list",
        ),
        (vec!["task", "open"], "omarchy-calendar task open ITEM_ID"),
        (vec!["event", "open", "task:123"], "Expected event:"),
        (vec!["task", "open", "event:123"], "Expected task:"),
        (
            vec!["account", "remove"],
            "omarchy-calendar account remove ACCOUNT_ID",
        ),
        (vec!["remove", "a1"], "Unknown command"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_omarchy-calendar"))
            .args(args)
            .env_remove("XDG_RUNTIME_DIR")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            result["error"]["message"]
                .as_str()
                .unwrap()
                .contains(expected),
            "{result}"
        );
    }
}
