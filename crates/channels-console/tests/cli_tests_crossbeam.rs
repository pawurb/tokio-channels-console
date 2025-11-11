#[cfg(test)]
pub mod tests {
    use std::process::Command;

    #[test]
    fn test_basic_output() {
        let output = Command::new("cargo")
            .args([
                "run",
                "-p",
                "channels-console-crossbeam-test",
                "--example",
                "basic_crossbeam",
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
            "examples/basic_crossbeam.rs",
            "hello-there",
            "unbounded",
            "bounded[10]",
            "bounded[1]",
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
                "channels-console-crossbeam-test",
                "--example",
                "closed_crossbeam",
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

        // Check that all three channels have "closed" state
        assert!(
            stdout.contains("closed-sender"),
            "Expected closed-sender channel in output"
        );
        assert!(
            stdout.contains("closed-receiver"),
            "Expected closed-receiver channel in output"
        );
        assert!(
            stdout.contains("closed-unbounded"),
            "Expected closed-unbounded channel in output"
        );
    }

    #[test]
    fn test_basic_json_output() {
        let output = Command::new("cargo")
            .args([
                "run",
                "-p",
                "channels-console-crossbeam-test",
                "--example",
                "basic_json_crossbeam",
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

        let all_expected = ["\"label\": \"bounded\"", "\"label\": \"unbounded\""];

        let stdout = String::from_utf8_lossy(&output.stdout);

        for expected in all_expected {
            assert!(
                stdout.contains(expected),
                "Expected:\n{expected}\n\nGot:\n{stdout}",
            );
        }
    }

    #[test]
    fn test_iter_output() {
        let output = Command::new("cargo")
            .args([
                "run",
                "-p",
                "channels-console-crossbeam-test",
                "--example",
                "iter_crossbeam",
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
            "bounded",
            "bounded-2",
            "bounded-3",
            "examples/iter_crossbeam.rs:16",
            "examples/iter_crossbeam.rs:16-2",
            "examples/iter_crossbeam.rs:16-3",
        ];

        for expected in all_expected {
            assert!(
                stdout.contains(expected),
                "Expected:\n{expected}\n\nGot:\n{stdout}",
            );
        }
    }

    #[test]
    fn test_data_endpoints() {
        use channels_console::SerializableChannelStats;
        use std::{process::Command, thread::sleep, time::Duration};

        // Spawn example process
        let mut child = Command::new("cargo")
            .args([
                "run",
                "-p",
                "channels-console-crossbeam-test",
                "--example",
                "basic_crossbeam",
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

        let all_expected = ["basic_crossbeam.rs", "hello-there"];
        for expected in all_expected {
            assert!(
                json_text.contains(expected),
                "Expected:\n{expected}\n\nGot:\n{json_text}",
            );
        }

        // Test /logs/:id endpoint
        let metrics: Vec<SerializableChannelStats> =
            serde_json::from_str(&json_text).expect("Failed to parse metrics JSON");

        if let Some(first_channel) = metrics.first() {
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
}
