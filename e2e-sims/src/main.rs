//! Hosts all three sims on ephemeral ports, announces them as one JSON line,
//! and dies with its parent (stdin EOF) so orphans can't outlive a test run.

use e2e_sims::{
    anki::anki_router,
    llm::llm_router,
    lrclib::lrclib_router,
    server::{SimState, serve},
};
use std::sync::Arc;
use tokio::io::AsyncReadExt;

macro_rules! start {
    ($router:expr) => {{
        let (inner, sim) = $router;
        let (reset, seed) = (sim.clone(), sim.clone());
        let state = Arc::new(SimState::new(
            Box::new(move || reset.reset()),
            Box::new(move |v| seed.seed(v)),
        ));
        serve(inner, state).await?.0
    }};
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let llm = start!(llm_router());
    let lrclib = start!(lrclib_router());
    let anki = start!(anki_router());

    // Hand-formatted so key order matches the harness's expected line verbatim.
    println!(r#"{{"llm": {llm}, "lrclib": {lrclib}, "anki": {anki}}}"#);

    // SIGTERM needs no handler: the default disposition already terminates us.
    let mut stdin = tokio::io::stdin();
    let mut buf = [0u8; 64];
    while let Ok(n) = stdin.read(&mut buf).await {
        if n == 0 {
            break;
        }
    }
    Ok(())
}
