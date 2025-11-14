use channels_console::SerializableChannelStats;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Stylize,
    symbols::border,
    text::Line,
    widgets::{Block, Paragraph, TableState},
    Frame,
};

use crate::cmd::console::app::{CachedLogs, Focus};

use super::channels::render_channels_panel;
use super::inspect::render_inspect_popup;
use super::logs::{render_logs_panel, render_logs_placeholder};

/// Renders the main content area including channels table, logs panel, and error states
#[allow(clippy::too_many_arguments)]
pub fn render_main_view(
    frame: &mut Frame,
    area: Rect,
    stats: &[SerializableChannelStats],
    error: &Option<String>,
    metrics_port: u16,
    table_state: &mut TableState,
    logs_table_state: &mut TableState,
    focus: Focus,
    show_logs: bool,
    logs: &Option<CachedLogs>,
    paused: bool,
    inspected_log: &Option<channels_console::LogEntry>,
    current_elapsed_ns: u64,
) {
    if let Some(ref error_msg) = error {
        if stats.is_empty() {
            let error_text = vec![
                Line::from(""),
                Line::from("Error").red().bold().centered(),
                Line::from(""),
                Line::from(error_msg.as_str()).red().centered(),
                Line::from(""),
                Line::from(format!(
                    "Make sure the metrics server is running on http://127.0.0.1:{}",
                    metrics_port
                ))
                .yellow()
                .centered(),
            ];

            let block = Block::bordered().border_set(border::THICK);
            frame.render_widget(Paragraph::new(error_text).block(block), area);
            return;
        }
    }

    if stats.is_empty() {
        let empty_text = vec![
            Line::from(""),
            Line::from("No channel statistics found")
                .yellow()
                .centered(),
            Line::from(""),
            Line::from("Make sure channels are instrumented and the server is running").centered(),
        ];

        let block = Block::bordered().border_set(border::THICK);
        frame.render_widget(Paragraph::new(empty_text).block(block), area);
        return;
    }

    // Split the area if logs are being shown
    let (table_area, logs_area) = if show_logs {
        let chunks = Layout::default()
            .direction(ratatui::layout::Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);
        (chunks[0], Some(chunks[1]))
    } else {
        (area, None)
    };

    let selected_index = table_state.selected().unwrap_or(0);
    let channel_position = selected_index + 1; // 1-indexed
    let total_channels = stats.len();

    render_channels_panel(
        stats,
        table_area,
        frame,
        table_state,
        show_logs,
        focus,
        channel_position,
        total_channels,
    );

    // Render logs panel if visible
    if let Some(logs_area) = logs_area {
        let channel_label = table_state
            .selected()
            .and_then(|i| stats.get(i))
            .map(|stat| {
                if stat.label.is_empty() {
                    stat.id.to_string()
                } else {
                    stat.label.clone()
                }
            })
            .unwrap_or_else(|| "Unknown".to_string());

        if let Some(ref cached_logs) = logs {
            let has_missing_log = cached_logs
                .logs
                .sent_logs
                .iter()
                .any(|entry| entry.message.is_none());
            let display_label = if has_missing_log {
                format!("{} (missing \"log = true\")", channel_label)
            } else {
                channel_label
            };
            render_logs_panel(
                cached_logs,
                &display_label,
                logs_area,
                frame,
                logs_table_state,
                focus == Focus::Logs,
                current_elapsed_ns,
            );
        } else {
            let message = if paused {
                "(refresh paused)"
            } else if error.is_some() {
                "(cannot fetch new data)"
            } else {
                "(no data)"
            };
            render_logs_placeholder(&channel_label, message, logs_area, frame);
        }
    }

    if focus == Focus::Inspect {
        if let Some(ref inspected_log) = inspected_log {
            render_inspect_popup(inspected_log, area, frame);
        }
    }
}
