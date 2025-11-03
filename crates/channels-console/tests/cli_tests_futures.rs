#[cfg(test)]
pub mod tests {
    use std::process::Command;

    #[test]
    fn test_basic_output() {
        let output = Command::new("cargo")
            .args([
                "run",
                "-p",
                "channels-console-futures-test",
                "--example",
                "basic_futures",
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
            "examples/basic_futures.rs",
            "oneshot-labeled",
            "bounded[10]",
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
                "channels-console-futures-test",
                "--example",
                "basic_json_futures",
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
            "\"label\": \"unbounded\"",
            "\"label\": \"bounded\"",
            "\"label\": \"oneshot\"",
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
                "channels-console-futures-test",
                "--example",
                "closed_futures",
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
                "channels-console-futures-test",
                "--example",
                "oneshot_closed_futures",
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

        let all_expected = ["| closed |", "oneshot-closed"];

        for expected in all_expected {
            assert!(
                stdout.contains(expected),
                "Expected:\n{expected}\n\nGot:\n{stdout}",
            );
        }
    }

    #[test]
    fn test_data_endpoints() {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        use channels_console::SerializableChannelStats;
        use std::{process::Command, thread::sleep, time::Duration};

        // Spawn example process
        let mut child = Command::new("cargo")
            .args([
                "run",
                "-p",
                "channels-console-futures-test",
                "--example",
                "basic_futures",
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
                Ok(response) => {
                    json_text = response
                        .into_string()
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

        let all_expected = [
            "basic_futures.rs",
            "oneshot-labeled",
            "bounded[10]",
            "notified",
        ];
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
            let encoded_id = URL_SAFE_NO_PAD.encode(first_channel.id.as_bytes());

            let logs_url = format!("http://127.0.0.1:6770/logs/{}", encoded_id);
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
