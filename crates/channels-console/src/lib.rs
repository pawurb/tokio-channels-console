use crossbeam_channel::{unbounded, Sender as CbSender};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Instant;

pub mod channels_guard;
pub use channels_guard::{ChannelsGuard, ChannelsGuardBuilder};

use crate::http_api::start_metrics_server;
mod http_api;
mod wrappers;

/// A single log entry for a message sent or received.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub index: u64,
    pub timestamp: u64,
    pub message: Option<String>,
}

impl LogEntry {
    pub(crate) fn new(index: u64, timestamp: Instant, message: Option<String>) -> Self {
        let start_time = START_TIME.get().copied().unwrap_or(timestamp);
        let timestamp_nanos = timestamp.duration_since(start_time).as_nanos() as u64;
        Self {
            index,
            timestamp: timestamp_nanos,
            message,
        }
    }
}

/// Type of a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelType {
    Bounded(usize),
    Unbounded,
    Oneshot,
}

impl std::fmt::Display for ChannelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelType::Bounded(size) => write!(f, "bounded[{}]", size),
            ChannelType::Unbounded => write!(f, "unbounded"),
            ChannelType::Oneshot => write!(f, "oneshot"),
        }
    }
}

impl Serialize for ChannelType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ChannelType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;

        match s.as_str() {
            "unbounded" => Ok(ChannelType::Unbounded),
            "oneshot" => Ok(ChannelType::Oneshot),
            _ => {
                // try: bounded[123]
                if let Some(inner) = s.strip_prefix("bounded[").and_then(|x| x.strip_suffix(']')) {
                    let size = inner
                        .parse()
                        .map_err(|_| serde::de::Error::custom("invalid bounded size"))?;
                    Ok(ChannelType::Bounded(size))
                } else {
                    Err(serde::de::Error::custom("invalid channel type"))
                }
            }
        }
    }
}

/// Format of the output produced by ChannelsGuard on drop.
#[derive(Clone, Copy, Debug, Default)]
pub enum Format {
    #[default]
    Table,
    Json,
    JsonPretty,
}

/// State of a instrumented channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelState {
    #[default]
    Active,
    Closed,
    Full,
    Notified,
}

impl std::fmt::Display for ChannelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl ChannelState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChannelState::Active => "active",
            ChannelState::Closed => "closed",
            ChannelState::Full => "full",
            ChannelState::Notified => "notified",
        }
    }
}

impl Serialize for ChannelState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ChannelState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "active" => Ok(ChannelState::Active),
            "closed" => Ok(ChannelState::Closed),
            "full" => Ok(ChannelState::Full),
            "notified" => Ok(ChannelState::Notified),
            _ => Err(serde::de::Error::custom("invalid channel state")),
        }
    }
}

/// Statistics for a single instrumented channel.
#[derive(Debug, Clone)]
pub(crate) struct ChannelStats {
    pub(crate) id: &'static str,
    pub(crate) label: Option<&'static str>,
    pub(crate) channel_type: ChannelType,
    pub(crate) state: ChannelState,
    pub(crate) sent_count: u64,
    pub(crate) received_count: u64,
    pub(crate) type_name: &'static str,
    pub(crate) type_size: usize,
    pub(crate) sent_logs: VecDeque<LogEntry>,
    pub(crate) received_logs: VecDeque<LogEntry>,
}

impl ChannelStats {
    pub fn queued(&self) -> u64 {
        self.sent_count.saturating_sub(self.received_count)
    }

    pub fn total_bytes(&self) -> u64 {
        self.sent_count * self.type_size as u64
    }

    pub fn queued_bytes(&self) -> u64 {
        self.queued() * self.type_size as u64
    }
}

/// Serializable version of channel statistics for JSON responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableChannelStats {
    pub id: String,
    pub label: String,
    pub channel_type: ChannelType,
    pub state: ChannelState,
    pub sent_count: u64,
    pub received_count: u64,
    pub queued: u64,
    pub type_name: String,
    pub type_size: usize,
    pub total_bytes: u64,
    pub queued_bytes: u64,
}

impl From<&ChannelStats> for SerializableChannelStats {
    fn from(stats: &ChannelStats) -> Self {
        let label = resolve_label(stats.id, stats.label);
        Self {
            id: stats.id.to_string(),
            label,
            channel_type: stats.channel_type,
            state: stats.state,
            sent_count: stats.sent_count,
            received_count: stats.received_count,
            queued: stats.queued(),
            type_name: stats.type_name.to_string(),
            type_size: stats.type_size,
            total_bytes: stats.total_bytes(),
            queued_bytes: stats.queued_bytes(),
        }
    }
}

impl ChannelStats {
    fn new(
        id: &'static str,
        label: Option<&'static str>,
        channel_type: ChannelType,
        type_name: &'static str,
        type_size: usize,
    ) -> Self {
        Self {
            id,
            label,
            channel_type,
            state: ChannelState::default(),
            sent_count: 0,
            received_count: 0,
            type_name,
            type_size,
            sent_logs: VecDeque::new(),
            received_logs: VecDeque::new(),
        }
    }

    /// Update the channel state based on sent/received counts.
    /// Sets state to Full if sent > received, otherwise Active (unless explicitly closed).
    fn update_state(&mut self) {
        if self.state == ChannelState::Closed || self.state == ChannelState::Notified {
            return;
        }

        if self.sent_count > self.received_count {
            self.state = ChannelState::Full;
        } else {
            self.state = ChannelState::Active;
        }
    }
}

/// Events sent to the background statistics collection thread.
#[derive(Debug)]
pub(crate) enum StatsEvent {
    Created {
        id: &'static str,
        display_label: Option<&'static str>,
        channel_type: ChannelType,
        type_name: &'static str,
        type_size: usize,
    },
    MessageSent {
        id: &'static str,
        log: Option<String>,
        timestamp: Instant,
    },
    MessageReceived {
        id: &'static str,
        timestamp: Instant,
    },
    Closed {
        id: &'static str,
    },
    #[allow(dead_code)]
    Notified {
        id: &'static str,
    },
}

type StatsState = (
    CbSender<StatsEvent>,
    Arc<RwLock<HashMap<&'static str, ChannelStats>>>,
);

/// Global state for statistics collection.
static STATS_STATE: OnceLock<StatsState> = OnceLock::new();

static START_TIME: OnceLock<Instant> = OnceLock::new();

const DEFAULT_LOG_LIMIT: usize = 50;

fn get_log_limit() -> usize {
    std::env::var("CHANNELS_CONSOLE_LOG_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_LOG_LIMIT)
}

/// Initialize the statistics collection system (called on first instrumented channel).
/// Returns a reference to the global state.
fn init_stats_state() -> &'static StatsState {
    STATS_STATE.get_or_init(|| {
        START_TIME.get_or_init(Instant::now);

        let (tx, rx) = unbounded::<StatsEvent>();
        let stats_map = Arc::new(RwLock::new(HashMap::<&'static str, ChannelStats>::new()));
        let stats_map_clone = Arc::clone(&stats_map);

        std::thread::Builder::new()
            .name("channel-stats-collector".into())
            .spawn(move || {
                while let Ok(event) = rx.recv() {
                    let mut stats = stats_map_clone.write().unwrap();
                    match event {
                        StatsEvent::Created {
                            id: key,
                            display_label,
                            channel_type,
                            type_name,
                            type_size,
                        } => {
                            stats.insert(
                                key,
                                ChannelStats::new(
                                    key,
                                    display_label,
                                    channel_type,
                                    type_name,
                                    type_size,
                                ),
                            );
                        }
                        StatsEvent::MessageSent { id, log, timestamp } => {
                            if let Some(channel_stats) = stats.get_mut(id) {
                                channel_stats.sent_count += 1;
                                channel_stats.update_state();

                                let limit = get_log_limit();
                                if channel_stats.sent_logs.len() >= limit {
                                    channel_stats.sent_logs.pop_front();
                                }
                                channel_stats.sent_logs.push_back(LogEntry::new(
                                    channel_stats.sent_count,
                                    timestamp,
                                    log,
                                ));
                            }
                        }
                        StatsEvent::MessageReceived { id, timestamp } => {
                            if let Some(channel_stats) = stats.get_mut(id) {
                                channel_stats.received_count += 1;
                                channel_stats.update_state();

                                let limit = get_log_limit();
                                if channel_stats.received_logs.len() >= limit {
                                    channel_stats.received_logs.pop_front();
                                }
                                channel_stats.received_logs.push_back(LogEntry::new(
                                    channel_stats.received_count,
                                    timestamp,
                                    None,
                                ));
                            }
                        }
                        StatsEvent::Closed { id } => {
                            if let Some(channel_stats) = stats.get_mut(id) {
                                channel_stats.state = ChannelState::Closed;
                            }
                        }
                        StatsEvent::Notified { id } => {
                            if let Some(channel_stats) = stats.get_mut(id) {
                                channel_stats.state = ChannelState::Notified;
                            }
                        }
                    }
                }
            })
            .expect("Failed to spawn channel-stats-collector thread");

        // Spawn the metrics HTTP server in the background
        // Check environment variable for custom port, default to 6770
        let port = std::env::var("CHANNELS_CONSOLE_METRICS_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(6770);
        let addr = format!("127.0.0.1:{}", port);

        std::thread::spawn(move || {
            start_metrics_server(&addr);
        });

        (tx, stats_map)
    })
}

fn resolve_label(id: &'static str, provided: Option<&'static str>) -> String {
    if let Some(l) = provided {
        return l.to_string();
    }
    if let Some(pos) = id.rfind(':') {
        let (path, line_part) = id.split_at(pos);
        let line = &line_part[1..];
        format!("{}:{}", extract_filename(path), line)
    } else {
        extract_filename(id)
    }
}

fn extract_filename(path: &str) -> String {
    let components: Vec<&str> = path.split('/').collect();
    if components.len() >= 2 {
        format!(
            "{}/{}",
            components[components.len() - 2],
            components[components.len() - 1]
        )
    } else {
        path.to_string()
    }
}

/// Format bytes into human-readable units (B, KB, MB, GB, TB).
pub fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }

    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    if unit_idx == 0 {
        format!("{} {}", bytes, UNITS[unit_idx])
    } else {
        format!("{:.1} {}", size, UNITS[unit_idx])
    }
}

/// Trait for instrumenting channels.
///
/// This trait is not intended for direct use. Use the `instrument!` macro instead.
#[doc(hidden)]
pub trait Instrument {
    type Output;
    fn instrument(
        self,
        channel_id: &'static str,
        label: Option<&'static str>,
        capacity: Option<usize>,
    ) -> Self::Output;
}

/// Trait for instrumenting channels with message logging.
///
/// This trait is not intended for direct use. Use the `instrument!` macro with `log = true` instead.
#[doc(hidden)]
pub trait InstrumentLog {
    type Output;
    fn instrument_log(
        self,
        channel_id: &'static str,
        label: Option<&'static str>,
        capacity: Option<usize>,
    ) -> Self::Output;
}

cfg_if::cfg_if! {
    if #[cfg(any(feature = "tokio", feature = "futures"))] {
        use std::sync::LazyLock;
        pub static RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_time()
                .build()
                .unwrap()
        });
    }
}

/// Instrument a channel creation to wrap it with debugging proxies.
/// Currently only supports bounded, unbounded and oneshot channels.
///
/// # Examples
///
/// ```
/// use tokio::sync::mpsc;
/// use channels_console::instrument;
///
/// #[tokio::main]
/// async fn main() {
///
///    // Create channels normally
///    let (tx, rx) = mpsc::channel::<String>(100);
///
///    // Instrument them only when the feature is enabled
///    #[cfg(feature = "channels-console")]
///    let (tx, rx) = channels_console::instrument!((tx, rx));
///
///    // The channel works exactly the same way
///    tx.send("Hello".to_string()).await.unwrap();
/// }
/// ```
///
/// By default, channels are labeled with their file location and line number (e.g., `src/worker.rs:25`). You can provide custom labels for easier identification:
///
/// ```rust,no_run
/// use tokio::sync::mpsc;
/// use channels_console::instrument;
/// let (tx, rx) = mpsc::channel::<String>(10);
/// #[cfg(feature = "channels-console")]
/// let (tx, rx) = channels_console::instrument!((tx, rx), label = "task-queue");
/// ```
///
/// # Important: Capacity Parameter
///
/// **For `std::sync::mpsc` and `futures::channel::mpsc` bounded channels**, you **must** specify the `capacity` parameter
/// because their APIs don't expose the capacity after creation:
///
/// ```rust,no_run
/// use std::sync::mpsc;
/// use channels_console::instrument;
///
/// // std::sync::mpsc::sync_channel - MUST specify capacity
/// let (tx, rx) = mpsc::sync_channel::<String>(10);
/// let (tx, rx) = instrument!((tx, rx), capacity = 10);
///
/// // With label
/// let (tx, rx) = mpsc::sync_channel::<String>(10);
/// let (tx, rx) = instrument!((tx, rx), label = "my-channel", capacity = 10);
/// ```
///
/// Tokio channels don't require this because their capacity is accessible from the channel handles.
///
/// **Message Logging:**
///
/// By default, instrumentation only tracks message timestamps. To capture the actual content of messages for debugging,
/// enable logging with the `log = true` parameter (the message type must implement `std::fmt::Debug`):
///
/// ```rust,no_run
/// use tokio::sync::mpsc;
/// use channels_console::instrument;
///
/// // Enable message logging (requires Debug trait on the message type)
/// let (tx, rx) = mpsc::channel::<String>(10);
/// #[cfg(feature = "channels-console")]
/// let (tx, rx) = channels_console::instrument!((tx, rx), log = true);
///
///
#[macro_export]
macro_rules! instrument {
    ($expr:expr) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        $crate::Instrument::instrument($expr, CHANNEL_ID, None, None)
    }};

    ($expr:expr, label = $label:literal) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        $crate::Instrument::instrument($expr, CHANNEL_ID, Some($label), None)
    }};

    ($expr:expr, capacity = $capacity:expr) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::Instrument::instrument($expr, CHANNEL_ID, None, Some($capacity))
    }};

    ($expr:expr, label = $label:literal, capacity = $capacity:expr) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::Instrument::instrument($expr, CHANNEL_ID, Some($label), Some($capacity))
    }};

    ($expr:expr, capacity = $capacity:expr, label = $label:literal) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::Instrument::instrument($expr, CHANNEL_ID, Some($label), Some($capacity))
    }};

    // Variants with log = true
    ($expr:expr, log = true) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        $crate::InstrumentLog::instrument_log($expr, CHANNEL_ID, None, None)
    }};

    ($expr:expr, label = $label:literal, log = true) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        $crate::InstrumentLog::instrument_log($expr, CHANNEL_ID, Some($label), None)
    }};

    ($expr:expr, log = true, label = $label:literal) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        $crate::InstrumentLog::instrument_log($expr, CHANNEL_ID, Some($label), None)
    }};

    ($expr:expr, capacity = $capacity:expr, log = true) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::InstrumentLog::instrument_log($expr, CHANNEL_ID, None, Some($capacity))
    }};

    ($expr:expr, log = true, capacity = $capacity:expr) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::InstrumentLog::instrument_log($expr, CHANNEL_ID, None, Some($capacity))
    }};

    ($expr:expr, label = $label:literal, capacity = $capacity:expr, log = true) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::InstrumentLog::instrument_log($expr, CHANNEL_ID, Some($label), Some($capacity))
    }};

    ($expr:expr, label = $label:literal, log = true, capacity = $capacity:expr) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::InstrumentLog::instrument_log($expr, CHANNEL_ID, Some($label), Some($capacity))
    }};

    ($expr:expr, capacity = $capacity:expr, label = $label:literal, log = true) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::InstrumentLog::instrument_log($expr, CHANNEL_ID, Some($label), Some($capacity))
    }};

    ($expr:expr, capacity = $capacity:expr, log = true, label = $label:literal) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::InstrumentLog::instrument_log($expr, CHANNEL_ID, Some($label), Some($capacity))
    }};

    ($expr:expr, log = true, label = $label:literal, capacity = $capacity:expr) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::InstrumentLog::instrument_log($expr, CHANNEL_ID, Some($label), Some($capacity))
    }};

    ($expr:expr, log = true, capacity = $capacity:expr, label = $label:literal) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::InstrumentLog::instrument_log($expr, CHANNEL_ID, Some($label), Some($capacity))
    }};
}

fn get_channel_stats() -> HashMap<&'static str, ChannelStats> {
    if let Some((_, stats_map)) = STATS_STATE.get() {
        stats_map.read().unwrap().clone()
    } else {
        HashMap::new()
    }
}

fn get_serializable_stats() -> Vec<SerializableChannelStats> {
    let mut stats: Vec<SerializableChannelStats> = get_channel_stats()
        .values()
        .map(SerializableChannelStats::from)
        .collect();

    stats.sort_by(|a, b| a.id.cmp(&b.id));
    stats
}

/// Serializable log response containing sent and received logs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelLogs {
    pub id: String,
    pub sent_logs: Vec<LogEntry>,
    pub received_logs: Vec<LogEntry>,
}

pub(crate) fn get_channel_logs(channel_id: &str) -> Option<ChannelLogs> {
    let stats = get_channel_stats();
    stats.get(channel_id).map(|channel_stats| {
        let mut sent_logs: Vec<LogEntry> = channel_stats.sent_logs.iter().cloned().collect();

        let mut received_logs: Vec<LogEntry> =
            channel_stats.received_logs.iter().cloned().collect();

        // Sort by index descending (most recent first)
        sent_logs.sort_by(|a, b| b.index.cmp(&a.index));
        received_logs.sort_by(|a, b| b.index.cmp(&a.index));

        ChannelLogs {
            id: channel_id.to_string(),
            sent_logs,
            received_logs,
        }
    })
}
