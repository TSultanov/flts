//! Headless FLTS sync node for multi-node integration tests.
//!
//! Embeds the *real* engine + roster/reconcile (`library::sync`) — everything a
//! desktop runs except the Tauri/WebView layer — and exposes a tiny HTTP control
//! API so a test runner (curl / a script) can drive it: read this device's ID,
//! pair a peer, create a book, and list devices/books to assert convergence.
//!
//! Networking is deterministic and discovery-free: each node binds a fixed BEP
//! port and a background loop pins every peer's address from its roster name
//! (`tcp://<name>:<bep_port>`). In Docker/compose the device name equals the
//! service hostname, so even mesh peers discovered via the roster connect
//! directly — no public or local discovery server needed. (Production uses
//! real discovery; address-pinning is test-only scaffolding.)

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use isolang::Language;
use library::library::Library;
use library::sync::control::OptionsPatch;
use library::sync::engine::{EngineConfig, SyncEngine};

struct Node {
    engine: Arc<SyncEngine>,
    library: Arc<Library>,
    bep_port: u16,
    name: String,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn main() -> Result<()> {
    let node_name = std::env::var("FLTS_NODE_NAME").context("FLTS_NODE_NAME required")?;
    let library_root =
        PathBuf::from(std::env::var("FLTS_LIBRARY_DIR").context("FLTS_LIBRARY_DIR required")?);
    let home = PathBuf::from(env_or("FLTS_SYNC_HOME", "/data/syncthing"));
    let control_port: u16 = env_or("FLTS_HARNESS_PORT", "8090").parse()?;
    let bep_port: u16 = env_or("FLTS_BEP_PORT", "22000").parse()?;

    let rt = tokio::runtime::Runtime::new()?;
    let node = Arc::new(rt.block_on(setup(node_name.clone(), library_root, home, bep_port))?);
    eprintln!(
        "[{node_name}] up: id={} control=:{control_port} bep=:{bep_port}",
        node.engine.my_id()
    );

    // Background: reconcile against the synced roster, then pin peer addresses.
    {
        let node = node.clone();
        rt.spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(3));
            loop {
                ticker.tick().await;
                if let Err(e) = node.engine.reconcile_once().await {
                    eprintln!("[{}] reconcile: {e}", node.name);
                }
                if let Err(e) = pin_addresses(&node).await {
                    eprintln!("[{}] pin: {e}", node.name);
                }
            }
        });
    }

    // Blocking control server on the main thread; each request blocks on the
    // runtime for its async work while the reconcile task runs on worker threads.
    let server = tiny_http::Server::http(("0.0.0.0", control_port))
        .map_err(|e| anyhow!("control server bind failed: {e}"))?;
    for mut req in server.incoming_requests() {
        let method = req.method().to_string();
        let url = req.url().to_string();
        let mut body = String::new();
        let _ = req.as_reader().read_to_string(&mut body);

        let result = rt.block_on(handle(&node, &method, &url, &body));
        let json = result.unwrap_or_else(|e| {
            format!("{{\"error\":\"{}\"}}", e.to_string().replace('"', "'"))
        });
        let header: tiny_http::Header =
            "Content-Type: application/json".parse().unwrap();
        let _ = req.respond(tiny_http::Response::from_string(json).with_header(header));
    }
    Ok(())
}

async fn setup(name: String, library_root: PathBuf, home: PathBuf, bep_port: u16) -> Result<Node> {
    std::fs::create_dir_all(&library_root)?;
    let library = Arc::new(Library::open(library_root.clone()).await?);

    // No discovery/relays/NAT; bind a known, routable port for static dialing.
    let options = OptionsPatch {
        global_discovery: false,
        local_discovery: false,
        relays: false,
        nat: false,
        listen_addresses: vec![format!("tcp://0.0.0.0:{bep_port}")],
    };
    let engine = Arc::new(
        SyncEngine::start(EngineConfig {
            home,
            library_root,
            options,
            loopback_only: false,
        })
        .await?,
    );
    engine.set_device_name(&name).await?;

    Ok(Node {
        engine,
        library,
        bep_port,
        name,
    })
}

/// Pins each known peer's connection address from its roster name.
async fn pin_addresses(node: &Node) -> Result<()> {
    let my_id = node.engine.my_id().to_string();
    let client = node.engine.client();
    for d in client.list_devices().await? {
        if d.device_id == my_id || d.name.is_empty() {
            continue;
        }
        let addr = format!("tcp://{}:{}", d.name, node.bep_port);
        client.set_device_addresses(&d.device_id, vec![addr]).await?;
    }
    Ok(())
}

async fn handle(node: &Node, method: &str, url: &str, body: &str) -> Result<String> {
    match (method, url) {
        ("GET", "/id") => Ok(format!("{{\"deviceId\":\"{}\"}}", node.engine.my_id())),

        ("GET", "/devices") => {
            let peers = node.engine.list_peers().await?;
            Ok(serde_json::to_string(&peers)?)
        }

        ("GET", "/books") => {
            let books = node.library.list_books().await?;
            let titles: Vec<String> = books.into_iter().map(|b| b.title).collect();
            Ok(serde_json::to_string(&titles)?)
        }

        ("POST", "/pair") => {
            let v: serde_json::Value = serde_json::from_str(body)?;
            let id = v["deviceId"]
                .as_str()
                .ok_or_else(|| anyhow!("deviceId required"))?;
            let name = v["name"].as_str().unwrap_or("device");
            node.engine.pair_device(id, name).await?;
            Ok("{\"ok\":true}".into())
        }

        ("POST", "/book") => {
            let v: serde_json::Value = serde_json::from_str(body)?;
            let title = v["title"].as_str().ok_or_else(|| anyhow!("title required"))?;
            let text = v["text"].as_str().unwrap_or("hello from the harness");
            let eng = Language::from_639_3("eng").expect("eng is a valid ISO 639-3 code");
            node.library.create_book_plain(title, text, &eng).await?;
            Ok("{\"ok\":true}".into())
        }

        _ => Err(anyhow!("not found: {method} {url}")),
    }
}
