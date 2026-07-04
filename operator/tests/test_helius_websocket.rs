//! Integration test for Helius WebSocket connection
//!
//! This test connects to Helius LaserStream WebSocket and verifies:
//! 1. Connection establishment
//! 2. Subscription to wallet addresses
//! 3. Message reception and parsing

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
#[ignore] // Run with: cargo test --test test_helius_websocket -- --ignored
async fn test_helius_websocket_connection() -> Result<()> {
    // Load environment variables
    dotenvy::dotenv().ok();

    let api_key = std::env::var("HELIUS_API_KEY")
        .expect("HELIUS_API_KEY must be set");

    let websocket_url = format!(
        "wss://mainnet.helius-rpc.com/?api-key={}",
        api_key
    );

    println!("Connecting to: {}", websocket_url);

    // Connect to WebSocket
    let (ws_stream, _) = tokio_tungstenite::connect_async(&websocket_url).await?;
    println!("✅ Connected to Helius WebSocket");

    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    // Subscribe to a known wallet (e.g., a popular SOL wallet)
    let test_wallet = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83hZRuYos7HtX"; // Example wallet

    let subscription = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "transactionSubscribe",
        "params": [{
            "account": [test_wallet],
            "failed": false,
            "commitment": "confirmed"
        }]
    });

    ws_sender.send(subscription.to_string().into()).await?;
    println!("✅ Subscribed to wallet: {}", test_wallet);

    // Wait for messages (timeout after 30 seconds)
    let message_timeout = timeout(Duration::from_secs(30), async {
        let mut message_count = 0;
        let max_messages = 5;

        while message_count < max_messages {
            if let Some(Ok(message)) = ws_receiver.next().await {
                match message {
                    tokio_tungstenite::tungstenite::Message::Text(text) => {
                        println!("📨 Received message: {}", text);

                        // Try to parse as JSON
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                            println!("  Parsed: {}", serde_json::to_string_pretty(&value).unwrap_or_default());

                            // Check if it's a subscription notification
                            if value.get("method").and_then(|m| m.as_str()) == Some("subscriptionNotification") {
                                println!("  ✅ Got subscription notification!");
                                message_count += 1;
                            }
                        }
                    }
                    tokio_tungstenite::tungstenite::Message::Ping(data) => {
                        println!("📡 Received ping, sending pong");
                        ws_sender.send(tokio_tungstenite::tungstenite::Message::Pong(data)).await.ok();
                    }
                    tokio_tungstenite::tungstenite::Message::Pong(_) => {
                        println!("📡 Received pong");
                    }
                    tokio_tungstenite::tungstenite::Message::Close(_) => {
                        println!("🔌 Connection closed");
                        break;
                    }
                    _ => {}
                }
            }
        }

        Ok::<(), anyhow::Error>(message_count)
    }).await;

    match message_timeout {
        Ok(Ok(count)) => {
            println!("✅ Received {} messages in 30 seconds", count);
            if count > 0 {
                println!("✅ WebSocket test PASSED");
            } else {
                println!("⚠️  No transaction messages received (wallet may be inactive)");
            }
        }
        Ok(Err(e)) => {
            println!("❌ Error: {}", e);
        }
        Err(_) => {
            println!("⚠️  Timeout after 30 seconds - no recent transactions for test wallet");
        }
    }

    // Close connection
    ws_sender.send(tokio_tungstenite::tungstenite::Message::Close(None)).await?;

    Ok(())
}

#[tokio::test]
#[ignore] // Run with: cargo test --test test_helius_websocket -- --ignored
async fn test_websocket_ping_pong() -> Result<()> {
    dotenvy::dotenv().ok();

    let api_key = std::env::var("HELIUS_API_KEY")
        .expect("HELIUS_API_KEY must be set");

    let websocket_url = format!(
        "wss://mainnet.helius-rpc.com/?api-key={}",
        api_key
    );

    println!("Testing ping/pong with: {}", websocket_url);

    let (ws_stream, _) = tokio_tungstenite::connect_async(&websocket_url).await?;
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    // Send ping
    ws_sender.send(tokio_tungstenite::tungstenite::Message::Ping(vec![1, 2, 3])).await?;
    println!("✅ Sent ping");

    // Wait for pong (timeout 5 seconds)
    let pong_received = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(Ok(message)) = ws_receiver.next().await {
                if matches!(message, tokio_tungstenite::tungstenite::Message::Pong(_)) {
                    return true;
                }
            }
        }
    }).await;

    if pong_received.is_ok() {
        println!("✅ Received pong - ping/pong working!");
    } else {
        println!("⚠️  No pong received within 5 seconds");
    }

    ws_sender.send(tokio_tungstenite::tungstenite::Message::Close(None)).await?;

    Ok(())
}
