use smol::Timer;
use std::time::Duration;

#[allow(unused_mut)]
fn main() {
    smol::block_on(async {
        #[cfg(feature = "channels-console")]
        let _channels_guard = channels_console::ChannelsGuard::new();

        let (tx, rx) = futures_channel::oneshot::channel::<String>();
        #[cfg(feature = "channels-console")]
        let (tx, rx) = channels_console::channel!((tx, rx), label = "oneshot-closed");

        drop(rx);

        Timer::after(Duration::from_millis(50)).await;

        match tx.send("Hello oneshot!".to_string()) {
            Ok(_) => panic!("Not expected: send succeeded"),
            Err(_) => println!("Expected: Failed to send"),
        }
        Timer::after(Duration::from_millis(100)).await;

        println!("\nExample completed!");
    })
}
