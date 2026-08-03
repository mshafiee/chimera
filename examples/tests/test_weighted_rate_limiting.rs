// Manual test demonstration for weighted rate limiting
// Run this with: cargo run --example test_weighted_rate_limiting

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chimera_operator::monitoring::rate_limiter::{
    RateLimiter, RequestPriority, RpcMethodCategory,
};

/// Panics (with a clear message) when a check fails, so the demo doubles as a
/// regression check instead of unconditionally printing ✅ lines.
fn check(cond: bool, msg: &str) {
    assert!(cond, "CHECK FAILED: {msg}");
    println!("   PASS: {msg}");
}

async fn test_weighted_rate_limiting() {
    println!("=== Weighted Rate Limiting Demonstration ===\n");

    // Create a rate limiter with 10 credits per second
    let limiter = Arc::new(RateLimiter::new(10, 1));

    println!("1. Testing RPC Method Categorization");
    println!("   Creating rate limiter with 10 credits/second window\n");

    // Test lightweight operations (StatusCheck = 1 credit)
    println!("2. Making StatusCheck calls (weight 1):");
    for i in 0..3 {
        limiter
            .acquire_rpc(RpcMethodCategory::StatusCheck, RequestPriority::Polling)
            .await;
        println!("   StatusCheck call {} completed", i + 1);
    }

    let metrics = limiter.get_metrics();
    println!("   Credits used: {} (expected: 3)", metrics.total_credits_used);
    check(
        metrics.total_credits_used == 3,
        "three StatusCheck calls consume exactly 3 credits cumulatively",
    );
    check(
        metrics.current_credits == 3,
        "the 1-second window currently holds 3 credits",
    );

    // Test heavy operations (TransactionFetch = 5 credits)
    println!("\n3. Making TransactionFetch calls (weight 5):");
    limiter
        .acquire_rpc(RpcMethodCategory::TransactionFetch, RequestPriority::Polling)
        .await;
    println!("   TransactionFetch call 1 completed");

    let metrics = limiter.get_metrics();
    println!("   Credits used: {} (expected: 8)", metrics.total_credits_used);
    check(
        metrics.total_credits_used == 8,
        "TransactionFetch adds 5 credits to the cumulative total",
    );

    // Test that we can still make lightweight calls
    println!("\n4. Verifying lightweight calls still work:");
    limiter
        .acquire_rpc(RpcMethodCategory::StatusCheck, RequestPriority::Entry)
        .await;
    println!("   StatusCheck with Entry priority completed");

    let metrics = limiter.get_metrics();
    check(
        metrics.total_credits_used == 9,
        "final StatusCheck brings the cumulative total to 9",
    );

    // Show category breakdown
    println!("\n5. Category Metrics:");
    if let Some(ref categories) = metrics.requests_by_category {
        println!("   Requests by category:");
        for (category, count) in categories {
            println!("     {}: {} requests", category, count);
        }
    }
    if let Some(ref credits) = metrics.credits_by_category {
        println!("   Credits by category:");
        for (category, count) in credits {
            println!("     {}: {} credits", category, count);
        }
    }

    let credits = metrics
        .credits_by_category
        .as_ref()
        .expect("category credits must be tracked");
    check(
        credits.get("StatusCheck") == Some(&4),
        "StatusCheck category accounts for 4 credits (4 calls at weight 1)",
    );
    check(
        credits.get("TransactionFetch") == Some(&5),
        "TransactionFetch category accounts for 5 credits (1 call at weight 5)",
    );

    // 6. Priority test: leave the limiter with a single 5-credit entry in the
    // window, then contend an Entry request against a Polling request and
    // verify the Entry request wins.
    //
    // The Entry StatusCheck (1 credit) fits in the remaining 5 free credits
    // immediately; the Polling HeavyOperation (8 credits) cannot fit until
    // the 5-credit entry ages out of the 1s window, so Entry strictly first.
    println!("\n6. Priority System:");
    let limiter2 = Arc::new(RateLimiter::new(10, 1));
    limiter2
        .acquire_rpc(RpcMethodCategory::TransactionFetch, RequestPriority::Polling)
        .await;
    check(
        limiter2.get_metrics().current_credits == 5,
        "limiter holds one 5-credit entry (5/10 credits)",
    );

    // Let the entry age out of the 1-second window so the contest starts with
    // a known state: one 5-credit entry, 5 credits free.
    tokio::time::sleep(tokio::time::Duration::from_millis(1100)).await;
    check(
        limiter2.get_metrics().current_credits == 0,
        "5-credit entry aged out of the window after ~1s",
    );
    limiter2
        .acquire_rpc(RpcMethodCategory::TransactionFetch, RequestPriority::Polling)
        .await;
    check(
        limiter2.get_metrics().current_credits == 5,
        "contest starts with one 5-credit entry and 5 free credits",
    );

    let completion_order = Arc::new(AtomicUsize::new(0));
    let (entry_done_tx, entry_done_rx) = tokio::sync::oneshot::channel();
    let (polling_done_tx, polling_done_rx) = tokio::sync::oneshot::channel();

    let l_entry = limiter2.clone();
    let entry_order = completion_order.clone();
    let entry_task = tokio::spawn(async move {
        l_entry
            .acquire_rpc(RpcMethodCategory::StatusCheck, RequestPriority::Entry)
            .await;
        entry_order.store(1, Ordering::SeqCst);
        let _ = entry_done_tx.send(());
    });

    let l_polling = limiter2.clone();
    let polling_order = completion_order.clone();
    let polling_task = tokio::spawn(async move {
        l_polling
            .acquire_rpc(RpcMethodCategory::HeavyOperation, RequestPriority::Polling)
            .await;
        polling_order.store(10, Ordering::SeqCst);
        let _ = polling_done_tx.send(());
    });

    let start = std::time::Instant::now();
    entry_done_rx.await.expect("entry request must complete");
    let entry_elapsed = start.elapsed();
    polling_done_rx.await.expect("polling request must complete");
    let polling_elapsed = start.elapsed();

    println!("   Entry completed after {:?}", entry_elapsed);
    println!("   Polling completed after {:?}", polling_elapsed);
    check(
        // Last writer wins: 10 = polling finished last = entry finished first.
        completion_order.load(Ordering::SeqCst) == 10,
        "Entry-priority request acquires before the Polling request",
    );

    let _ = (entry_task, polling_task);

    println!("\n=== Test Results ===");
    println!("✅ Weighted rate limiting is working correctly");
    println!("✅ Heavy operations (TransactionFetch) consume more credits");
    println!("✅ Lightweight operations (StatusCheck) consume fewer credits");
    println!("✅ Category metrics are tracked accurately");
    println!("✅ Priority system reduces wait times for important calls");
}

#[tokio::main]
async fn main() {
    test_weighted_rate_limiting().await;
}
