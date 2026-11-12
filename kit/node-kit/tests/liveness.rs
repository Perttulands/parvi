//! A client that stays silent through the node's pings (a device asleep or
//! off the network) is taken for gone, so what it held is given back, while
//! one that answers stays. Its own binary: the pings here are fast.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use parvi_core::frame::Kinds;
use parvi_node_kit::{Area, BoxFuture, Built, Channel, Client, Config, Listing, Options, Setup};

/// Whether each client's holding has been given back, in connection order.
static HELD: std::sync::Mutex<Vec<Arc<AtomicBool>>> = std::sync::Mutex::new(Vec::new());

struct Holding;

impl Area for Holding {
    fn channel(&self, _client: Client) -> Option<Box<dyn Channel>> {
        let held = Arc::new(AtomicBool::new(true));
        HELD.lock().unwrap().push(held.clone());
        Some(Box::new(Hold(held)))
    }
}

/// What a client holds while connected, like a terminal's sizing seat.
struct Hold(Arc<AtomicBool>);

impl Drop for Hold {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Channel for Hold {
    fn receive(&mut self, _frame: Vec<u8>) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }

    fn next(&mut self) -> BoxFuture<'_, Option<Vec<u8>>> {
        Box::pin(std::future::pending())
    }
}

const KINDS: Kinds = &[0x70..=0x70];

fn holding(_: &Setup) -> anyhow::Result<Built> {
    Built::now(Holding)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_silent_client_is_let_go_and_an_answering_one_kept() {
    parvi_node_kit::set_liveness(Duration::from_millis(100), Duration::from_millis(400));
    let dir = std::env::temp_dir().join(format!("parvi-liveness-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let listing = Listing {
        name: "holding",
        kinds: KINDS,
        commands: &[],
        build: holding,
    };
    let node = parvi_node_kit::start(
        Options {
            config: Config::parse("", dir.clone()).unwrap(),
            dir,
            listen: Some("127.0.0.1:0".into()),
        },
        &[listing],
    )
    .await
    .unwrap();
    let url = format!("ws://{}/ws", node.address);

    // Silent: read its hello, then never read again, so never answer.
    let (mut silent, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    silent.next().await.unwrap().unwrap();
    // Answering: keeps reading, which answers each ping.
    let (mut answering, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let reading = tokio::spawn(async move { while let Some(Ok(_)) = answering.next().await {} });

    let given_back = |i: usize| !HELD.lock().unwrap()[i].load(Ordering::SeqCst);
    for _ in 0..100 {
        if given_back(0) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(given_back(0), "the silent client still holds what it held");
    assert!(!given_back(1), "the client answering pings was let go");
    reading.abort();
    drop(silent);
    node.stop().await;
}
