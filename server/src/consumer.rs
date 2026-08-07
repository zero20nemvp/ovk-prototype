use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::flow::{Entry, FlowClient};
use crate::store::Store;

/// Resident consumer of the rapids. Crash-safe at-least-once semantics,
/// mirroring OVK's own FlowTCP#consume: the cursor position means "last FULLY
/// processed ordinal" — we get() the next entry cursor-free, ingest it into
/// the store, and only then commit the cursor. A crash between ingest and
/// commit reprocesses one entry on restart; every ingest is idempotent
/// (INSERT .. DO NOTHING / keyed UPDATE), so that is harmless.
pub fn run(flow: FlowClient, store: Arc<Mutex<Store>>, cursor: String) {
    // An empty enrichment store with an advanced durable cursor means the
    // local DB was wiped (or is new on this box): every screen is derivable
    // from the rapids, so drop the cursor and replay from origin.
    let mut reset_done = false;
    loop {
        if reset_done {
            break;
        }
        let empty = store
            .lock()
            .expect("store mutex poisoned")
            .run_count()
            .map(|n| n == 0)
            .unwrap_or(false);
        match flow.cursor_position(&cursor) {
            Ok(Some(p)) if empty && p > 0 => {
                eprintln!("consumer: store empty but cursor at {p} — replaying from origin");
                if let Err(e) = flow.cursor_delete(&cursor) {
                    eprintln!("consumer: cursor reset failed: {e:#}");
                }
                reset_done = true;
            }
            Ok(_) => reset_done = true,
            Err(e) => {
                eprintln!("consumer: waiting for flow daemon: {e:#}");
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }
    if let Err(e) = flow.cursor_create(&cursor) {
        eprintln!("consumer: cannot create cursor '{cursor}': {e:#}");
    }
    loop {
        match step(&flow, &store, &cursor) {
            Ok(true) => {}                                        // ingested one; go again
            Ok(false) => std::thread::sleep(Duration::from_millis(1500)), // at head
            Err(e) => {
                eprintln!("consumer: {e:#}");
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }
}

fn step(flow: &FlowClient, store: &Arc<Mutex<Store>>, cursor: &str) -> anyhow::Result<bool> {
    let pos = flow.cursor_position(cursor)?;
    let next = pos.map_or(0, |p| p + 1);
    let Some(entry) = flow.get(next)? else {
        return Ok(false);
    };
    ingest(store, &entry)?;
    flow.cursor_commit(cursor, next)?;
    Ok(true)
}

fn ingest(store: &Arc<Mutex<Store>>, e: &Entry) -> anyhow::Result<()> {
    let st = store.lock().expect("store mutex poisoned");
    let ct = e.content_type.as_str();
    if ct == "content:query" {
        st.upsert_run(e.ordinal, &e.body, e.enqueued_at)?;
    } else if let Some(persona) = ct.strip_prefix("reply:") {
        st.attach_reply(e.ordinal, persona, &e.body, e.enqueued_at)?;
    } else if let Some(item) = ct.strip_prefix("prediction:") {
        match item.parse::<u64>() {
            Ok(item) => st.set_prediction(item, e.ordinal, &e.body)?,
            Err(_) => eprintln!("prediction with non-numeric item tag '{item}' — skipped"),
        }
    } else if let Some(item) = ct.strip_prefix("outcome:") {
        match item.parse::<u64>() {
            Ok(item) => st.set_outcome(item, e.ordinal, &e.body)?,
            Err(_) => eprintln!("outcome with non-numeric item tag '{item}' — skipped"),
        }
    }
    // Anything else on the rapids (outcome:*, future types) is simply not ours
    // to render yet; the cursor still advances past it.
    Ok(())
}
