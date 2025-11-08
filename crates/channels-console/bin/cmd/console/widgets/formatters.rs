use channels_console::ChannelType;
use ratatui::{
    style::{Color, Style},
    widgets::Cell,
};

/// Truncates a string from the left, adding "..." prefix if needed
pub(crate) fn truncate_left(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let truncated_len = max_len.saturating_sub(3);
        let start_idx = s.len().saturating_sub(truncated_len);
        format!("...{}", &s[start_idx..])
    }
}

/// Creates a colored usage bar cell showing queue capacity
pub(crate) fn usage_bar(queued: u64, channel_type: &ChannelType, _width: usize) -> Cell<'static> {
    let capacity = match channel_type {
        ChannelType::Bounded(cap) => Some(*cap),
        ChannelType::Oneshot => Some(1),
        ChannelType::Unbounded => None,
    };

    match capacity {
        Some(cap) if cap > 0 => {
            let percentage = (queued as f64 / cap as f64 * 100.0).min(100.0);

            let text = format!("[{}/{}]", queued, cap);

            let color = if percentage >= 100.0 {
                Color::Red
            } else if percentage >= 50.0 {
                Color::Yellow
            } else {
                Color::Green
            };

            Cell::from(text).style(Style::default().fg(color))
        }
        _ => Cell::from("N/A"),
    }
}

/// Formats a delay in nanoseconds to a human-readable string
pub(crate) fn format_delay(delay_ns: u64) -> String {
    if delay_ns < 1_000 {
        format!("{}ns", delay_ns)
    } else if delay_ns < 1_000_000 {
        format!("{:.1}μs", delay_ns as f64 / 1_000.0)
    } else if delay_ns < 1_000_000_000 {
        format!("{:.2}ms", delay_ns as f64 / 1_000_000.0)
    } else {
        format!("{:.3}s", delay_ns as f64 / 1_000_000_000.0)
    }
}

/// Truncates a message with ellipsis suffix, padding to max_len
pub(crate) fn truncate_message(msg: &str, max_len: usize) -> String {
    if msg.len() <= max_len {
        format!("{:<width$}", msg, width = max_len)
    } else {
        let truncated = &msg[..max_len.saturating_sub(3)];
        format!("{}...", truncated)
    }
}
