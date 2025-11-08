use channels_console::{ChannelLogs, SerializableChannelStats};
use eyre::Result;

/// Fetches channel metrics from the HTTP server
pub(crate) fn fetch_metrics(agent: &ureq::Agent, port: u16) -> Result<Vec<SerializableChannelStats>> {
    let url = format!("http://127.0.0.1:{}/metrics", port);
    let response = agent.get(&url).call()?;
    let stats: Vec<SerializableChannelStats> = response.into_json()?;
    Ok(stats)
}

/// Fetches logs for a specific channel from the HTTP server
pub(crate) fn fetch_logs(agent: &ureq::Agent, port: u16, channel_id: u64) -> Result<ChannelLogs> {
    let url = format!("http://127.0.0.1:{}/logs/{}", port, channel_id);
    let response = agent.get(&url).call()?;
    let logs: ChannelLogs = response.into_json()?;
    Ok(logs)
}
