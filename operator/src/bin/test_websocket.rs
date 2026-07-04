//! Standalone WebSocket test - run with: cargo run --bin test_websocket

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables
    dotenvy::dotenv().ok();

    let api_key = std::env::var("HELIUS_API_KEY")
        .expect("HELIUS_API_KEY must be set in .env file");

    let websocket_url = format!(
        "wss://mainnet.helius-rpc.com/?api-key={}",
        api_key
    );

    println!("🔌 Connecting to Helius WebSocket...");
    println!("   URL: {}", &websocket_url[..40]);

    // Connect to WebSocket
    let (ws_stream, response) = tokio_tungstenite::connect_async(&websocket_url).await?;
    println!("✅ Connected! Status: {:?}", response.status());

    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    // Test 1: Ping/Pong
    println!("\n📡 Test 1: Ping/Pong");
    ws_sender.send(tokio_tungstenite::tungstenite::Message::Ping(vec![1, 2, 3])).await?;
    println!("   Sent ping");

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
        println!("   ✅ Received pong - ping/pong working!");
    } else {
        println!("   ⚠️  No pong received within 5 seconds");
    }

    // Test 2: Try standard accountSubscribe first
    println!("\n📝 Test 2: Standard accountSubscribe");
    let test_wallet = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83hZRuYos7HtX";
    println!("   Wallet: {}", test_wallet);

    // Try standard Solana accountSubscribe first
    let standard_subscription = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "accountSubscribe",
        "params": [
            test_wallet,
            {
                "encoding": "jsonParsed",
                "commitment": "confirmed"
            }
        ]
    });

    ws_sender.send(standard_subscription.to_string().into()).await?;
    println!("   ✅ Standard accountSubscribe request sent");

    // Test 3: Try Helius accountSubscribe with filters
    println!("\n📝 Test 3: Helius accountSubscribe with filters");
    let helius_subscription = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "accountSubscribe",
        "params": [
            test_wallet,
            {
                "encoding": "jsonParsed",
                "commitment": "confirmed",
                "filters": ["transfer", "nativeTransfer"]
            }
        ]
    });

    ws_sender.send(helius_subscription.to_string().into()).await?;
    println!("   ✅ Helius accountSubscribe request sent");

    // Test 4: Wait for messages
    println!("\n📨 Test 4: Waiting for messages (30 seconds)");
    println!("   Note: If wallet is inactive, no messages will be received");

    let message_timeout = timeout(Duration::from_secs(30), async {
        let mut message_count = 0;
        let mut subscription_notifications = 0;

        loop {
            if let Some(Ok(message)) = ws_receiver.next().await {
                match message {
                    tokio_tungstenite::tungstenite::Message::Text(text) => {
                        message_count += 1;
                        println!("   📨 Message #{}: {}", message_count, &text[..text.len().min(100)]);

                        // Try to parse as JSON
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                            // Check if it's a subscription notification
                            if value.get("method").and_then(|m| m.as_str()) == Some("subscriptionNotification") {
                                subscription_notifications += 1;
                                println!("      ✅ Subscription notification detected!");

                                // Try to extract transaction info
                                if let Some(params) = value.get("params") {
                                    if let Some(result) = params.get("result") {
                                        if let Some(transaction) = result.get("transaction") {
                                            if let Some(signature) = transaction.get("signature") {
                                                println!("      📝 Transaction: {}", signature.as_str().unwrap_or("unknown").chars().take(16).collect::<String>());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    tokio_tungstenite::tungstenite::Message::Ping(data) => {
                        println!("   📡 Received ping, sending pong");
                        ws_sender.send(tokio_tungstenite::tungstenite::Message::Pong(data)).await.ok();
                    }
                    tokio_tungstenite::tungstenite::Message::Pong(_) => {
                        println!("   📡 Received pong");
                    }
                    tokio_tungstenite::tungstenite::Message::Close(_) => {
                        println!("   🔌 Connection closed by server");
                        break;
                    }
                    _ => {}
                }
            }
        }

        Ok::<(usize, usize), anyhow::Error>((message_count, subscription_notifications))
    }).await;

    match message_timeout {
        Ok(Ok((total_msgs, sub_notifications))) => {
            println!("\n📊 Results:");
            println!("   Total messages: {}", total_msgs);
            println!("   Subscription notifications: {}", sub_notifications);

            if sub_notifications > 0 {
                println!("   ✅ WebSocket test PASSED - receiving transactions!");
            } else {
                println!("   ℹ️  No transaction notifications (wallet may be inactive)");
            }
        }
        Ok(Err(e)) => {
            println!("   ❌ Error: {}", e);
        }
        Err(_) => {
            println!("   ⏱️  Timeout after 30 seconds");
            println!("   ℹ️  This is normal for inactive wallets");
        }
    }

    // Close connection
    println!("\n🔌 Closing connection...");
    ws_sender.send(tokio_tungstenite::tungstenite::Message::Close(None)).await?;
    println!("✅ Connection closed gracefully");

    Ok(())
}
