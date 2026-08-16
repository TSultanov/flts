use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[tokio::test]
async fn binary_announces_three_ports_and_exits_on_stdin_close() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_flts-e2e-sims"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let ports: Value = serde_json::from_str(line.trim()).unwrap_or_else(|e| {
        let _ = child.kill();
        panic!("bad announce line {line:?}: {e}");
    });

    let obj = ports.as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["anki", "llm", "lrclib"], "{ports}");

    let c = reqwest::Client::new();
    for (name, port) in obj {
        let port = port.as_u64().unwrap_or_else(|| panic!("{name}: {ports}"));
        let r = c
            .get(format!("http://127.0.0.1:{port}/_sim/requests"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{name}");
        assert_eq!(r.json::<Value>().await.unwrap(), serde_json::json!([]));
    }

    drop(child.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if child.try_wait().unwrap().is_some() {
            return;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("binary did not exit within 2s of stdin close");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
