#[allow(unused_mut)]
fn main() {
    #[cfg(feature = "channels-console")]
    let _channels_guard = channels_console::ChannelsGuard::new();

    let (txa, _rxa) = crossbeam_channel::unbounded::<i32>();

    #[cfg(feature = "channels-console")]
    let (txa, _rxa) = channels_console::channel!((txa, _rxa), log = true);

    let (txb, rxb) = crossbeam_channel::bounded::<i32>(10);
    #[cfg(feature = "channels-console")]
    let (txb, rxb) = channels_console::channel!((txb, rxb), capacity = 10);

    let (txc, rxc) = crossbeam_channel::bounded::<String>(1);
    #[cfg(feature = "channels-console")]
    let (txc, rxc) = channels_console::channel!((txc, rxc), label = "hello-there", capacity = 1);

    let sender_handle = std::thread::spawn(move || {
        for i in 1..=3 {
            println!("[Sender] Sending message: {}", i);
            txa.send(i).expect("Failed to send");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        for i in 1..=3 {
            println!("[Sender] Sending message: {}", i);
            txb.send(i).expect("Failed to send");
            std::thread::sleep(std::time::Duration::from_millis(250));
        }

        println!("[Sender] Done sending messages");
    });

    let bounded_receiver_handle = std::thread::spawn(move || match rxc.recv() {
        Ok(msg) => println!("[Bounded-1] Received: {}", msg),
        Err(_) => println!("[Bounded-1] Sender dropped"),
    });

    println!("[Bounded-1] Sending message");
    txc.send("Hello from bounded channel!".to_string())
        .expect("Failed to send");

    sender_handle.join().expect("Sender thread failed");
    bounded_receiver_handle
        .join()
        .expect("Bounded receiver thread failed");

    #[cfg(feature = "channels-console")]
    drop(_channels_guard);

    while let Ok(msg) = rxb.recv() {
        println!("[Receiver] Received message: {}", msg);
    }

    println!("\nExample completed!");
}
