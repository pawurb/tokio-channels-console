#[cfg(test)]
pub mod tests {
    use std::process::Command;

    #[test]
    fn test_basic_streams_output() {
        let output = Command::new("cargo")
            .args([
                "run",
                "-p",
                "channels-console-streams-test",
                "--example",
                "basic_streams",
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
            "number-stream",
            "text-stream",
            "repeat-stream",
            "Stream example completed!",
            "Streams:",
            "5", // number-stream yielded 5 items
            "4", // text-stream yielded 4 items
            "3", // repeat-stream yielded 3 items
            "Yielded",
        ];

        for expected in all_expected {
            assert!(
                stdout.contains(expected),
                "Expected:\n{expected}\n\nGot:\n{stdout}",
            );
        }
    }

    #[test]
    fn test_streams_http_endpoints() {
        use std::{process::Command, thread::sleep, time::Duration};

        // Spawn example process
        let mut child = Command::new("cargo")
            .args([
                "run",
                "-p",
                "channels-console-streams-test",
                "--example",
                "basic_streams",
                "--features",
                "channels-console",
            ])
            .spawn()
            .expect("Failed to spawn command");

        let mut json_text = String::new();
        let mut last_error = None;

        // Test /streams endpoint
        for _attempt in 0..6 {
            sleep(Duration::from_millis(800));

            match ureq::get("http://127.0.0.1:6770/streams").call() {
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
            panic!("Failed after 6 retries: {}", error);
        }

        // Check for stream labels in JSON
        let all_expected = ["number-stream", "text-stream", "repeat-stream"];
        for expected in all_expected {
            assert!(
                json_text.contains(expected),
                "Expected:\n{expected}\n\nGot:\n{json_text}",
            );
        }

        // Parse and verify structure
        let streams_json: channels_console::StreamsJson =
            serde_json::from_str(&json_text).expect("Failed to parse streams JSON");

        assert_eq!(
            streams_json.streams.len(),
            3,
            "Expected 3 streams in response"
        );

        // Test /streams/:id/logs endpoint for a stream with logging enabled
        if let Some(stream_stat) = streams_json
            .streams
            .iter()
            .find(|s| s.label == "text-stream")
        {
            let logs_url = format!("http://127.0.0.1:6770/streams/{}/logs", stream_stat.id);
            let response = ureq::get(&logs_url)
                .call()
                .expect("Failed to call /streams/:id/logs endpoint");

            assert_eq!(
                response.status(),
                200,
                "Expected status 200 for /streams/:id/logs endpoint"
            );

            let logs_text = response
                .into_string()
                .expect("Failed to read logs response");

            assert!(logs_text.contains("logs"));
        }

        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn test_streams_closed_state() {
        let output = Command::new("cargo")
            .args([
                "run",
                "-p",
                "channels-console-streams-test",
                "--example",
                "basic_streams",
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

        // All streams should be in closed state after completion
        let closed_count = stdout.matches("| closed").count();
        assert!(
            closed_count >= 3,
            "Expected at least 3 'closed' states for streams, found {}.\nOutput:\n{}",
            closed_count,
            stdout
        );
    }
}
