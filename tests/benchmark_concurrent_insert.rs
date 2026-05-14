use harux::config::StoreConfig;
use harux::hl7::types::Hl7Message;
use harux::store::MessageStore;
use std::time::Instant;
use tokio::task::JoinHandle;

#[tokio::test]
async fn benchmark_concurrent_insert_p99() {
    let store = MessageStore::new(StoreConfig {
        max_messages: 1000,
        max_memory_mb: 10,
    });

    let mut handles: Vec<JoinHandle<Vec<u128>>> = Vec::new();
    let num_tasks = 10;
    let inserts_per_task = 500; // Will trigger several evictions (10 * 500 = 5000 inserts > 1000 cap)

    for t in 0..num_tasks {
        let store = store.clone();
        let handle = tokio::spawn(async move {
            let mut latencies = Vec::with_capacity(inserts_per_task);
            for i in 0..inserts_per_task {
                let msg_id = format!("bench-{}-{}", t, i);
                // Creating relatively large payloads to simulate MDM messages
                let mut msg = Hl7Message::new_empty("A".repeat(1024), "127.0.0.1".to_string());
                msg.id = msg_id;

                let start = Instant::now();
                store.insert(msg).await;
                let elapsed = start.elapsed().as_micros();
                latencies.push(elapsed);

                // Allow some context switching to actually benchmark concurrency
                if i % 10 == 0 {
                    tokio::task::yield_now().await;
                }
            }
            latencies
        });
        handles.push(handle);
    }

    let mut all_latencies = Vec::new();
    for handle in handles {
        let mut lats = handle.await.unwrap();
        all_latencies.append(&mut lats);
    }

    all_latencies.sort_unstable();

    let p50_idx = (all_latencies.len() as f64 * 0.50) as usize;
    let p95_idx = (all_latencies.len() as f64 * 0.95) as usize;
    let p99_idx = (all_latencies.len() as f64 * 0.99) as usize;

    println!("Total inserts: {}", all_latencies.len());
    println!("P50 Latency: {} us", all_latencies[p50_idx]);
    println!("P95 Latency: {} us", all_latencies[p95_idx]);
    println!("P99 Latency: {} us", all_latencies[p99_idx]);

    // Sleep a bit to let the background eviction task finish
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // We expect the P99 latency to be extremely low now (e.g. under 1ms),
    // because eviction happens asynchronously. We assert < 100_000us (100ms) to avoid flakiness in CI.
    assert!(
        all_latencies[p99_idx] < 100_000,
        "P99 latency is too high: {} us",
        all_latencies[p99_idx]
    );
}
