use smol::Timer;
use std::time::Duration;

struct Actor {
    name: String,
}

#[allow(unused_mut)]
fn main() {
    smol::block_on(async {
        let actor1 = Actor {
            name: "Actor 1".to_string(),
        };

        #[cfg(feature = "channels-console")]
        let _channels_guard = channels_console::ChannelsGuard::new();

        println!("Creating channels in loops...\n");

        println!("Creating 3 unbounded channels:");
        for i in 0..3 {
            let (tx, mut rx) = futures_channel::mpsc::unbounded::<i32>();

            #[cfg(feature = "channels-console")]
            let (tx, mut rx) = channels_console::instrument!((tx, rx), label = actor1.name.clone());

            println!("  - Created unbounded channel {}", i);

            smol::spawn(async move {
                tx.unbounded_send(i).expect("Failed to send");
                let _ = rx.try_next();
            })
            .detach();
        }

        println!("\nCreating 3 bounded channels:");
        for i in 0..3 {
            let (mut tx, mut rx) = futures_channel::mpsc::channel::<i32>(10);

            #[cfg(feature = "channels-console")]
            let (mut tx, mut rx) =
                channels_console::instrument!((tx, rx), capacity = 10, label = "bounded");

            println!("  - Created bounded channel {}", i);

            smol::spawn(async move {
                tx.try_send(i).expect("Failed to send");
                let _ = rx.try_next();
            })
            .detach();
        }

        println!("\nCreating 3 oneshot channels:");
        for i in 0..3 {
            let (tx, rx) = futures_channel::oneshot::channel::<String>();

            #[cfg(feature = "channels-console")]
            let (tx, rx) = channels_console::instrument!((tx, rx));

            println!("  - Created oneshot channel {}", i);

            smol::spawn(async move {
                let _ = tx.send(format!("Message {}", i));
                let _ = rx.await;
            })
            .detach();
        }

        Timer::after(Duration::from_millis(500)).await;

        println!("\nAll channels created and used!");

        #[cfg(feature = "channels-console")]
        drop(_channels_guard);

        println!("\nExample completed!");
    })
}
