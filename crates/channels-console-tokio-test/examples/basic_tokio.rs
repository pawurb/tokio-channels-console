struct Actor {
    name: String,
}

#[allow(unused_mut)]
#[tokio::main]
async fn main() {
    let actor1 = Actor {
        name: "Actor 1".to_string(),
    };

    #[cfg(feature = "channels-console")]
    let _channels_guard = channels_console::ChannelsGuard::new();

    let (txa, mut _rxa) = tokio::sync::mpsc::unbounded_channel::<i32>();

    #[cfg(feature = "channels-console")]
    let (txa, _rxa) = channels_console::instrument!((txa, _rxa), log = true, label = actor1.name);

    let (txb, mut rxb) = tokio::sync::mpsc::channel::<i32>(10);
    #[cfg(feature = "channels-console")]
    let (txb, mut rxb) = channels_console::instrument!((txb, rxb), label = "bounded-channel");

    let (txc, rxc) = tokio::sync::oneshot::channel::<String>();
    #[cfg(feature = "channels-console")]
    let (txc, rxc) = channels_console::instrument!((txc, rxc), label = "hello-there");

    let sender_handle = tokio::spawn(async move {
        for i in 1..=3 {
            println!("[Sender] Sending message: {}", i);
            txa.send(i).expect("Failed to send");
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        for i in 1..=3 {
            println!("[Sender] Sending message: {}", i);
            txb.send(i).await.expect("Failed to send");
            tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
        }

        println!("[Sender] Done sending messages");
    });

    let oneshot_receiver_handle = tokio::spawn(async move {
        match rxc.await {
            Ok(msg) => println!("[Oneshot] Received: {}", msg),
            Err(_) => println!("[Oneshot] Sender dropped"),
        }
    });

    println!("[Oneshot] Sending message");
    txc.send("Hello from oneshot!".to_string())
        .expect("Failed to send oneshot");

    sender_handle.await.expect("Sender task failed");
    oneshot_receiver_handle
        .await
        .expect("Oneshot receiver task failed");

    #[cfg(feature = "channels-console")]
    drop(_channels_guard);

    while let Some(msg) = rxb.recv().await {
        println!("[Receiver] Received message: {}", msg);
    }

    println!("\nExample completed!");
}
