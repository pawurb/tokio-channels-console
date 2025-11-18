#[tokio::main]
async fn main() {
    #[cfg(feature = "channels-console")]
    let _channels_guard = channels_console::ChannelsGuard::new();

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();

    #[cfg(feature = "channels-console")]
    let (tx, rx) = channels_console::channel!((tx, rx));

    drop(rx);

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    match tx.send("Hello oneshot!".to_string()) {
        Ok(_) => panic!("Not expected: send succeeded"),
        Err(_) => println!("Expected: Failed to send"),
    }
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    println!("\nExample completed!");
}
