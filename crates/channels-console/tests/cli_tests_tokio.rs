#[cfg(test)]
pub mod tests {
    use std::process::Command;

    #[test]
    fn test_basic_output() {
        let output = Command::new("cargo")
            .args([
                "run",
                "-p",
                "channels-console-tokio-test",
                "--example",
                "basic_tokio",
                "--features",
                "channels-console",
            ])
            .output()
            .expect("Failed to execute command");

        assert!(
            output.status.success(),
            "Command failed with status: {}",
            output.status
        );

        assert!(!output.stderr.is_empty(), "Stderr is empty");
        let all_expected = [
            "Actor 1",
            "bounded-channel",
            "hello-there",
            "unbounded",
            "bounded[10]",
            "oneshot",
            "notified",
        ];

        let stdout = String::from_utf8_lossy(&output.stdout);
        for expected in all_expected {
            assert!(
                stdout.contains(expected),
                "Expected:\n{expected}\n\nGot:\n{stdout}",
            );
        }
    }

    #[test]
    fn test_basic_json_output() {
        let output = Command::new("cargo")
            .args([
                "run",
                "-p",
                "channels-console-tokio-test",
                "--example",
                "basic_json_tokio",
                "--features",
                "channels-console",
            ])
            .output()
            .expect("Failed to execute command");

        assert!(
            output.status.success(),
            "Command failed with status: {}",
            output.status
        );

        let all_expected = [
            "\"label\": \"examples/basic_json_tokio.rs:",
            "\"label\": \"hello-there\"",
        ];

        let stdout = String::from_utf8_lossy(&output.stdout);

        for expected in all_expected {
            assert!(
                stdout.contains(expected),
                "Expected:\n{expected}\n\nGot:\n{stdout}",
            );
        }
    }

    #[test]
    fn test_closed_channels_output() {
        let output = Command::new("cargo")
            .args([
                "run",
                "-p",
                "channels-console-tokio-test",
                "--example",
                "closed_tokio",
                "--features",
                "channels-console",
            ])
            .output()
            .expect("Failed to execute command");

        assert!(
            output.status.success(),
            "Command failed with status: {}",
            output.status
        );

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Match "closed" with flexible spacing (table cells are padded)
        let closed_count = stdout.matches("| closed").count();
        assert_eq!(
            closed_count, 2,
            "Expected 'closed' state to appear 2 times in table (bounded and unbounded), found {}.\nOutput:\n{}",
            closed_count, stdout
        );

        let notified_count = stdout.matches("| notified").count();
        assert_eq!(
            notified_count, 1,
            "Expected 'notified' state to appear 1 time in table (oneshot), found {}.\nOutput:\n{}",
            notified_count, stdout
        );
    }

    #[test]
    fn test_oneshot_closed_output() {
        let output = Command::new("cargo")
            .args([
                "run",
                "-p",
                "channels-console-tokio-test",
                "--example",
                "oneshot_closed_tokio",
                "--features",
                "channels-console",
            ])
            .output()
            .expect("Failed to execute command");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "Command failed with status: {}\nStdout:\n{}\nStderr:\n{}",
            output.status,
            stdout,
            stderr
        );

        let all_expected = ["| closed |", "oneshot_closed_tokio.rs:"];

        for expected in all_expected {
            assert!(
                stdout.contains(expected),
                "Expected:\n{expected}\n\nGot:\n{stdout}",
            );
        }
    }

    #[test]
    fn test_data_endpoints() {
        use std::{process::Command, thread::sleep, time::Duration};

        // Spawn example process
        let mut child = Command::new("cargo")
            .args([
                "run",
                "-p",
                "channels-console-tokio-test",
                "--example",
                "basic_tokio",
                "--features",
                "channels-console",
            ])
            .spawn()
            .expect("Failed to spawn command");

        let mut json_text = String::new();
        let mut last_error = None;

        // Test /metrics endpoint
        for _attempt in 0..4 {
            sleep(Duration::from_millis(500));

            match ureq::get("http://127.0.0.1:6770/metrics").call() {
                Ok(mut response) => {
                    json_text = response
                        .body_mut()
                        .read_to_string()
                        .expect("Failed to read response body");
                    last_error = None;
                    break;
                }
                Err(e) => {
                    last_error = Some(format!("Request error: {}", e));
                }
            }
        }

        if let Some(error) = last_error {
            let _ = child.kill();
            panic!("Failed after 4 retries: {}", error);
        }

        let all_expected = ["Actor 1", "bounded-channel", "hello-there"];
        for expected in all_expected {
            assert!(
                json_text.contains(expected),
                "Expected:\n{expected}\n\nGot:\n{json_text}",
            );
        }

        // Test /logs/:id endpoint
        let metrics: channels_console::MetricsJson =
            serde_json::from_str(&json_text).expect("Failed to parse metrics JSON");

        if let Some(first_channel) = metrics.stats.first() {
            let logs_url = format!("http://127.0.0.1:6770/logs/{}", first_channel.id);
            let response = ureq::get(&logs_url)
                .call()
                .expect("Failed to call /logs/:id endpoint");

            assert_eq!(
                response.status(),
                200,
                "Expected status 200 for /logs/:id endpoint"
            );
        }

        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn test_iter_output() {
        let output = Command::new("cargo")
            .args([
                "run",
                "-p",
                "channels-console-tokio-test",
                "--example",
                "iter_tokio",
                "--features",
                "channels-console",
            ])
            .output()
            .expect("Failed to execute command");

        assert!(
            output.status.success(),
            "Command failed with status: {}",
            output.status
        );

        let stdout = String::from_utf8_lossy(&output.stdout);

        let all_expected = [
            "Actor 1",
            "Actor 1-2",
            "Actor 1-3",
            "examples/iter_tokio.rs:37",
            "examples/iter_tokio.rs:37-2",
            "examples/iter_tokio.rs:37-3",
            "examples/iter_tokio.rs:52",
            "examples/iter_tokio.rs:52-2",
            "examples/iter_tokio.rs:52-3",
        ];

        for expected in all_expected {
            assert!(
                stdout.contains(expected),
                "Expected:\n{expected}\n\nGot:\n{stdout}",
            );
        }
    }

    #[test]
    fn test_slow_consumer_no_panic() {
        let output = Command::new("cargo")
            .args([
                "run",
                "-p",
                "channels-console-tokio-test",
                "--example",
                "slow_consumer_tokio",
                "--features",
                "channels-console",
            ])
            .output()
            .expect("Failed to execute command");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "Command failed with status: {}\nStdout:\n{}\nStderr:\n{}",
            output.status,
            stdout,
            stderr
        );

        assert!(
            stdout.contains("Slow consumer example completed!"),
            "Expected completion message not found.\nOutput:\n{}",
            stdout
        );
    }
}
