use channels_console::{ChannelLogs, LogEntry};
use std::collections::HashMap;

/// Represents which UI component has focus
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Channels,
    Logs,
    Inspect,
}

/// Cached logs with a lookup map for received entries
pub(crate) struct CachedLogs {
    pub(crate) logs: ChannelLogs,
    pub(crate) received_map: HashMap<u64, LogEntry>,
}
