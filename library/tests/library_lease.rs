use library::library::Library;
use std::{
    io::{BufRead, Write},
    process::{Command, Stdio},
};

#[test]
fn lease_child_process() {
    let Some(root) = std::env::var_os("FLTS_LEASE_TEST_ROOT") else {
        return;
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let library = runtime.block_on(Library::open(root.into())).unwrap();
    println!("FLTS_LEASE_READY");
    std::io::stdout().flush().unwrap();
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    runtime.block_on(library.close()).unwrap();
}

#[tokio::test]
async fn separate_processes_exclude_each_other_and_process_exit_releases_lease() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("library");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lease_child_process", "--nocapture"])
        .env("FLTS_LEASE_TEST_ROOT", &root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut output = std::io::BufReader::new(stdout);
    loop {
        let mut line = String::new();
        assert!(
            output.read_line(&mut line).unwrap() > 0,
            "lease child exited early"
        );
        if line.contains("FLTS_LEASE_READY") {
            break;
        }
    }
    let competing = Library::open(root.clone()).await;
    assert!(
        competing
            .err()
            .unwrap()
            .to_string()
            .contains("library already open")
    );
    child.kill().unwrap();
    child.wait().unwrap();
    let library = Library::open(root).await.unwrap();
    library.close().await.unwrap();
}

#[tokio::test]
async fn concurrent_close_callers_observe_completion_before_reopening() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("library");
    let library = Library::open(root.clone()).await.unwrap();
    let id = library
        .create_book_plain("Closed", "source", &isolang::Language::Eng)
        .await
        .unwrap();
    let old = library.get_book(&id).await.unwrap();
    let (first, second) = tokio::join!(library.close(), library.close());
    first.unwrap();
    second.unwrap();
    let replacement = Library::open(root).await.unwrap();
    assert!(old.update_folder_path(vec![]).await.is_err());
    replacement.close().await.unwrap();
}
