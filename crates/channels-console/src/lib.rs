use crossbeam_channel::{unbounded, Sender as CbSender};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::AtomicU64;
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
    pub(crate) id: u64,
    pub(crate) source: &'static str,
    pub(crate) label: Option<String>,
    pub(crate) channel_type: ChannelType,
    pub(crate) state: ChannelState,
    pub(crate) sent_count: u64,
    pub(crate) received_count: u64,
    pub(crate) type_name: &'static str,
    pub(crate) type_size: usize,
    pub(crate) sent_logs: VecDeque<LogEntry>,
    pub(crate) received_logs: VecDeque<LogEntry>,
    pub(crate) iter: u32,
}

impl ChannelStats {
    pub fn queued(&self) -> u64 {
        self.sent_count
            .saturating_sub(self.received_count)
            .saturating_sub(1)
    }

    pub fn queued_bytes(&self) -> u64 {
        self.queued() * self.type_size as u64
    }
}

/// Wrapper for metrics JSON response containing stats and current time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsJson {
    /// Current elapsed time since program start in nanoseconds
    pub current_elapsed_ns: u64,
    /// Channel statistics
    pub stats: Vec<SerializableChannelStats>,
}

/// Serializable version of channel statistics for JSON responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableChannelStats {
    pub id: u64,
    pub source: String,
    pub label: String,
    pub has_custom_label: bool,
    pub channel_type: ChannelType,
    pub state: ChannelState,
    pub sent_count: u64,
    pub received_count: u64,
    pub queued: u64,
    pub type_name: String,
    pub type_size: usize,
    pub queued_bytes: u64,
    pub iter: u32,
}

impl From<&ChannelStats> for SerializableChannelStats {
    fn from(stats: &ChannelStats) -> Self {
        let label = resolve_label(stats.source, stats.label.as_deref(), stats.iter);

        Self {
            id: stats.id,
            source: stats.source.to_string(),
            label,
            has_custom_label: stats.label.is_some(),
            channel_type: stats.channel_type,
            state: stats.state,
            sent_count: stats.sent_count,
            received_count: stats.received_count,
            queued: stats.queued(),
            type_name: stats.type_name.to_string(),
            type_size: stats.type_size,
            queued_bytes: stats.queued_bytes(),
            iter: stats.iter,
        }
    }
}

impl ChannelStats {
    fn new(
        id: u64,
        source: &'static str,
        label: Option<String>,
        channel_type: ChannelType,
        type_name: &'static str,
        type_size: usize,
        iter: u32,
    ) -> Self {
        Self {
            id,
            source,
            label,
            channel_type,
            state: ChannelState::default(),
            sent_count: 0,
            received_count: 0,
            type_name,
            type_size,
            sent_logs: VecDeque::new(),
            received_logs: VecDeque::new(),
            iter,
        }
    }

    fn update_state(&mut self) {
        if self.state == ChannelState::Closed || self.state == ChannelState::Notified {
            return;
        }

        let queued = self.queued();
        let is_full = match self.channel_type {
            ChannelType::Bounded(cap) => queued >= cap as u64,
            ChannelType::Oneshot => queued >= 1,
            ChannelType::Unbounded => false,
        };

        if is_full {
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
        id: u64,
        source: &'static str,
        display_label: Option<String>,
        channel_type: ChannelType,
        type_name: &'static str,
        type_size: usize,
    },
    MessageSent {
        id: u64,
        log: Option<String>,
        timestamp: Instant,
    },
    MessageReceived {
        id: u64,
        timestamp: Instant,
    },
    Closed {
        id: u64,
    },
    #[allow(dead_code)]
    Notified {
        id: u64,
    },
}

type StatsState = (
    CbSender<StatsEvent>,
    Arc<RwLock<HashMap<u64, ChannelStats>>>,
);

/// Global state for statistics collection.
static STATS_STATE: OnceLock<StatsState> = OnceLock::new();

static START_TIME: OnceLock<Instant> = OnceLock::new();

/// Global counter for assigning unique IDs to channels.
pub(crate) static CHANNEL_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

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
        let stats_map = Arc::new(RwLock::new(HashMap::<u64, ChannelStats>::new()));
        let stats_map_clone = Arc::clone(&stats_map);

        std::thread::Builder::new()
            .name("channel-stats-collector".into())
            .spawn(move || {
                while let Ok(event) = rx.recv() {
                    let mut stats = stats_map_clone.write().unwrap();
                    match event {
                        StatsEvent::Created {
                            id,
                            source,
                            display_label,
                            channel_type,
                            type_name,
                            type_size,
                        } => {
                            // Count existing channels with the same source location
                            let iter =
                                stats.values().filter(|cs| cs.source == source).count() as u32;

                            stats.insert(
                                id,
                                ChannelStats::new(
                                    id,
                                    source,
                                    display_label,
                                    channel_type,
                                    type_name,
                                    type_size,
                                    iter,
                                ),
                            );
                        }
                        StatsEvent::MessageSent { id, log, timestamp } => {
                            if let Some(channel_stats) = stats.get_mut(&id) {
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
                            if let Some(channel_stats) = stats.get_mut(&id) {
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
                            if let Some(channel_stats) = stats.get_mut(&id) {
                                channel_stats.state = ChannelState::Closed;
                            }
                        }
                        StatsEvent::Notified { id } => {
                            if let Some(channel_stats) = stats.get_mut(&id) {
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

fn resolve_label(id: &'static str, provided: Option<&str>, iter: u32) -> String {
    let base_label = if let Some(l) = provided {
        l.to_string()
    } else if let Some(pos) = id.rfind(':') {
        let (path, line_part) = id.split_at(pos);
        let line = &line_part[1..];
        format!("{}:{}", extract_filename(path), line)
    } else {
        extract_filename(id)
    };

    if iter > 0 {
        format!("{}-{}", base_label, iter + 1)
    } else {
        base_label
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
        source: &'static str,
        label: Option<String>,
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
        source: &'static str,
        label: Option<String>,
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
/// ## Capacity Parameter
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
/// ## Message Logging
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

    ($expr:expr, label = $label:expr) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        $crate::Instrument::instrument($expr, CHANNEL_ID, Some($label.to_string()), None)
    }};

    ($expr:expr, capacity = $capacity:expr) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::Instrument::instrument($expr, CHANNEL_ID, None, Some($capacity))
    }};

    ($expr:expr, label = $label:expr, capacity = $capacity:expr) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::Instrument::instrument($expr, CHANNEL_ID, Some($label.to_string()), Some($capacity))
    }};

    ($expr:expr, capacity = $capacity:expr, label = $label:expr) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::Instrument::instrument($expr, CHANNEL_ID, Some($label.to_string()), Some($capacity))
    }};

    // Variants with log = true
    ($expr:expr, log = true) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        $crate::InstrumentLog::instrument_log($expr, CHANNEL_ID, None, None)
    }};

    ($expr:expr, label = $label:expr, log = true) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        $crate::InstrumentLog::instrument_log($expr, CHANNEL_ID, Some($label.to_string()), None)
    }};

    ($expr:expr, log = true, label = $label:expr) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        $crate::InstrumentLog::instrument_log($expr, CHANNEL_ID, Some($label.to_string()), None)
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

    ($expr:expr, label = $label:expr, capacity = $capacity:expr, log = true) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::InstrumentLog::instrument_log(
            $expr,
            CHANNEL_ID,
            Some($label.to_string()),
            Some($capacity),
        )
    }};

    ($expr:expr, label = $label:expr, log = true, capacity = $capacity:expr) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::InstrumentLog::instrument_log(
            $expr,
            CHANNEL_ID,
            Some($label.to_string()),
            Some($capacity),
        )
    }};

    ($expr:expr, capacity = $capacity:expr, label = $label:expr, log = true) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::InstrumentLog::instrument_log(
            $expr,
            CHANNEL_ID,
            Some($label.to_string()),
            Some($capacity),
        )
    }};

    ($expr:expr, capacity = $capacity:expr, log = true, label = $label:expr) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::InstrumentLog::instrument_log(
            $expr,
            CHANNEL_ID,
            Some($label.to_string()),
            Some($capacity),
        )
    }};

    ($expr:expr, log = true, label = $label:expr, capacity = $capacity:expr) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::InstrumentLog::instrument_log(
            $expr,
            CHANNEL_ID,
            Some($label.to_string()),
            Some($capacity),
        )
    }};

    ($expr:expr, log = true, capacity = $capacity:expr, label = $label:expr) => {{
        const CHANNEL_ID: &'static str = concat!(file!(), ":", line!());
        const _: usize = $capacity;
        $crate::InstrumentLog::instrument_log(
            $expr,
            CHANNEL_ID,
            Some($label.to_string()),
            Some($capacity),
        )
    }};
}

fn get_channel_stats() -> HashMap<u64, ChannelStats> {
    if let Some((_, stats_map)) = STATS_STATE.get() {
        stats_map.read().unwrap().clone()
    } else {
        HashMap::new()
    }
}

/// Compare two ChannelStats for sorting.
/// Custom labels come first (sorted alphabetically), then auto-generated labels (sorted by source and iter).
fn compare_channel_stats(a: &ChannelStats, b: &ChannelStats) -> std::cmp::Ordering {
    match (a.label.is_some(), b.label.is_some()) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (true, true) => a
            .label
            .as_ref()
            .unwrap()
            .cmp(b.label.as_ref().unwrap())
            .then_with(|| a.iter.cmp(&b.iter)),
        (false, false) => a.source.cmp(b.source).then_with(|| a.iter.cmp(&b.iter)),
    }
}

pub(crate) fn get_sorted_channel_stats() -> Vec<ChannelStats> {
    let mut stats: Vec<ChannelStats> = get_channel_stats().into_values().collect();
    stats.sort_by(compare_channel_stats);
    stats
}

fn get_metrics_json() -> MetricsJson {
    let stats = get_sorted_channel_stats()
        .iter()
        .map(SerializableChannelStats::from)
        .collect();

    let current_elapsed_ns = START_TIME
        .get()
        .expect("START_TIME must be initialized")
        .elapsed()
        .as_nanos() as u64;

    MetricsJson {
        current_elapsed_ns,
        stats,
    }
}

/// Serializable log response containing sent and received logs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelLogs {
    pub id: String,
    pub sent_logs: Vec<LogEntry>,
    pub received_logs: Vec<LogEntry>,
}

pub(crate) fn get_channel_logs(channel_id: &str) -> Option<ChannelLogs> {
    let id = channel_id.parse::<u64>().ok()?;
    let stats = get_channel_stats();
    stats.get(&id).map(|channel_stats| {
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
