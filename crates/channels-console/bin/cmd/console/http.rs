use channels_console::{ChannelLogs, ChannelsJson, StreamsJson};
use eyre::Result;

/// Fetches channel metrics from the HTTP server
pub(crate) fn fetch_channels(agent: &ureq::Agent, port: u16) -> Result<ChannelsJson> {
    let url = format!("http://127.0.0.1:{}/channels", port);
    let channels: ChannelsJson = agent.get(&url).call()?.body_mut().read_json()?;
    Ok(channels)
}

/// Fetches stream metrics from the HTTP server
pub(crate) fn fetch_streams(agent: &ureq::Agent, port: u16) -> Result<StreamsJson> {
    let url = format!("http://127.0.0.1:{}/streams", port);
    let streams: StreamsJson = agent.get(&url).call()?.body_mut().read_json()?;
    Ok(streams)
}

/// Fetches logs for a specific channel from the HTTP server
pub(crate) fn fetch_channel_logs(
    agent: &ureq::Agent,
    port: u16,
    channel_id: u64,
) -> Result<ChannelLogs> {
    let url = format!("http://127.0.0.1:{}/channels/{}/logs", port, channel_id);
    let logs: ChannelLogs = agent.get(&url).call()?.body_mut().read_json()?;
    Ok(logs)
}
