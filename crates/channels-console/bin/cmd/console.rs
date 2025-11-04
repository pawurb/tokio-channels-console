use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use channels_console::{
    format_bytes, ChannelLogs, ChannelState, ChannelType, LogEntry, SerializableChannelStats,
};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use eyre::Result;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    symbols::border,
    text::Line,
    widgets::{Block, Cell, Row, Table, Widget},
    DefaultTerminal, Frame,
};
use std::io;
use std::time::{Duration, Instant};

#[derive(Debug, Parser)]
pub struct ConsoleArgs {
    /// Port for the metrics server
    #[arg(long, default_value = "6770")]
    pub metrics_port: u16,
}

struct CachedLogs {
    logs: ChannelLogs,
    received_map: std::collections::HashMap<u64, LogEntry>,
}

pub struct App {
    stats: Vec<SerializableChannelStats>,
    error: Option<String>,
    exit: bool,
    last_refresh: Instant,
    last_successful_fetch: Option<Instant>,
    metrics_port: u16,
    last_render_duration: Duration,
    selected_index: usize,
    show_logs: bool,
    logs: Option<CachedLogs>,
    paused: bool,
    agent: ureq::Agent,
}

impl ConsoleArgs {
    pub fn run(&self) -> Result<()> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_millis(2000))
            .timeout_read(Duration::from_millis(1500))
            .build();

        let mut app = App {
            stats: Vec::new(),
            error: None,
            exit: false,
            last_refresh: Instant::now(),
            last_successful_fetch: None,
            metrics_port: self.metrics_port,
            last_render_duration: Duration::from_millis(0),
            selected_index: 0,
            show_logs: false,
            logs: None,
            paused: false,
            agent,
        };

        let mut terminal = ratatui::init();
        let app_result = app.run(&mut terminal);
        ratatui::restore();
        app_result.map_err(|e| eyre::eyre!("TUI error: {}", e))
    }
}

fn fetch_metrics(agent: &ureq::Agent, port: u16) -> Result<Vec<SerializableChannelStats>> {
    let url = format!("http://127.0.0.1:{}/metrics", port);
    let response = agent.get(&url).call()?;
    let stats: Vec<SerializableChannelStats> = response.into_json()?;
    Ok(stats)
}

fn fetch_logs(agent: &ureq::Agent, port: u16, channel_id: &str) -> Result<ChannelLogs> {
    let encoded_id = URL_SAFE_NO_PAD.encode(channel_id.as_bytes());
    let url = format!("http://127.0.0.1:{}/logs/{}", port, encoded_id);
    let response = agent.get(&url).call()?;
    let logs: ChannelLogs = response.into_json()?;
    Ok(logs)
}

fn truncate_left(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let truncated_len = max_len.saturating_sub(3);
        let start_idx = s.len().saturating_sub(truncated_len);
        format!("...{}", &s[start_idx..])
    }
}

fn usage_bar(queued: u64, channel_type: &ChannelType, width: usize) -> Cell<'static> {
    let capacity = match channel_type {
        ChannelType::Bounded(cap) => Some(*cap),
        ChannelType::Oneshot => Some(1),
        ChannelType::Unbounded => None,
    };

    match capacity {
        Some(cap) if cap > 0 => {
            let percentage = (queued as f64 / cap as f64 * 100.0).min(100.0);
            let filled = ((queued as f64 / cap as f64) * width as f64).round() as usize;
            let filled = filled.min(width);
            let empty = width.saturating_sub(filled);

            let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));
            let text = format!("{} {:>3.0}%", bar, percentage);

            let color = if percentage >= 100.0 {
                Color::Red
            } else if percentage >= 50.0 {
                Color::Yellow
            } else {
                Color::Green
            };

            Cell::from(text).style(Style::default().fg(color))
        }
        _ => Cell::from(format!("{} N/A", "░".repeat(width))),
    }
}

impl App {
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        const REFRESH_INTERVAL: Duration = Duration::from_millis(200);

        self.refresh_data();

        while !self.exit {
            if !self.paused && self.last_refresh.elapsed() >= REFRESH_INTERVAL {
                self.refresh_data();
            }

            let render_start = Instant::now();
            terminal.draw(|frame| self.draw(frame))?;
            self.last_render_duration = render_start.elapsed();

            self.handle_events()?;
        }
        Ok(())
    }

    fn refresh_data(&mut self) {
        match fetch_metrics(&self.agent, self.metrics_port) {
            Ok(stats) => {
                self.stats = stats;
                self.error = None;
                self.last_successful_fetch = Some(Instant::now());

                if self.selected_index >= self.stats.len() && !self.stats.is_empty() {
                    self.selected_index = self.stats.len() - 1;
                }

                if self.show_logs {
                    self.refresh_logs();
                }
            }
            Err(e) => {
                self.error = Some(format!("Failed to fetch metrics: {}", e));
            }
        }
        self.last_refresh = Instant::now();
    }

    fn draw(&self, frame: &mut Frame) {
        frame.render_widget(self, frame.area());
    }

    fn handle_events(&mut self) -> io::Result<()> {
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key_event) = event::read()? {
                if key_event.kind == KeyEventKind::Press {
                    self.handle_key_event(key_event);
                }
            }
        }
        Ok(())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => self.exit(),
            KeyCode::Up | KeyCode::Char('k') => self.select_previous(),
            KeyCode::Down | KeyCode::Char('j') => self.select_next(),
            KeyCode::Char('l') | KeyCode::Char('L') => self.toggle_logs(),
            KeyCode::Char('p') | KeyCode::Char('P') => self.toggle_pause(),
            _ => {}
        }
    }

    fn select_previous(&mut self) {
        if !self.stats.is_empty() {
            self.selected_index = self.selected_index.saturating_sub(1);

            if self.paused && self.show_logs {
                self.logs = None;
            } else if self.show_logs {
                self.refresh_logs();
            }
        }
    }

    fn select_next(&mut self) {
        if !self.stats.is_empty() {
            self.selected_index = (self.selected_index + 1).min(self.stats.len() - 1);

            if self.paused && self.show_logs {
                self.logs = None;
            } else if self.show_logs {
                self.refresh_logs();
            }
        }
    }

    fn toggle_logs(&mut self) {
        if !self.stats.is_empty() && self.selected_index < self.stats.len() {
            if self.show_logs {
                self.hide_logs();
            } else {
                self.show_logs = true;
                if self.paused {
                    self.logs = None;
                } else {
                    self.refresh_logs();
                }
            }
        }
    }

    fn hide_logs(&mut self) {
        self.show_logs = false;
        self.logs = None;
    }

    fn refresh_logs(&mut self) {
        if self.paused {
            return;
        }

        self.logs = None;

        if !self.stats.is_empty() && self.selected_index < self.stats.len() {
            let channel_id = &self.stats[self.selected_index].id;
            if let Ok(logs) = fetch_logs(&self.agent, self.metrics_port, channel_id) {
                let received_map: std::collections::HashMap<u64, LogEntry> = logs
                    .received_logs
                    .iter()
                    .map(|entry| (entry.index, entry.clone()))
                    .collect();

                self.logs = Some(CachedLogs { logs, received_map });
            }
        }
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;
    }

    fn exit(&mut self) {
        self.exit = true;
    }
}

impl Widget for &App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title = Line::from(" Channels Console ".bold());

        let refresh_status = if self.paused {
            "⏸ PAUSED ".to_string()
        } else if let Some(last_fetch) = self.last_successful_fetch {
            let elapsed = Instant::now().duration_since(last_fetch);
            let seconds = elapsed.as_secs();

            let is_stale = self.error.is_some() && !self.stats.is_empty();

            if is_stale {
                format!("⚠ {}s ", seconds)
            } else {
                format!("🔄 {}s ", seconds)
            }
        } else {
            String::new()
        };

        let bottom_line = if !refresh_status.is_empty() {
            Line::from(vec![
                " Quit ".into(),
                "<Q> ".blue().bold(),
                " | ".into(),
                "<↑↓/jk> ".blue().bold(),
                " | Logs ".into(),
                "<L> ".blue().bold(),
                " | Pause ".into(),
                "<P> ".blue().bold(),
                " | ".into(),
                refresh_status.yellow(),
            ])
        } else {
            Line::from(vec![
                " Quit ".into(),
                "<Q> ".blue().bold(),
                " | ".into(),
                "<↑↓/jk> ".blue().bold(),
                " | Logs ".into(),
                "<L> ".blue().bold(),
                " | Pause ".into(),
                "<P> ".blue().bold(),
            ])
        };

        #[cfg(feature = "dev")]
        let block = {
            let render_time_ms = self.last_render_duration.as_millis();
            let render_time_text = if render_time_ms < 10 {
                format!("  {}ms ", render_time_ms)
            } else {
                format!(" {}ms ", render_time_ms)
            };

            Block::bordered()
                .title(title.centered())
                .title_bottom(bottom_line.centered())
                .title_bottom(Line::from(render_time_text).cyan().right_aligned())
                .border_set(border::THICK)
        };

        #[cfg(not(feature = "dev"))]
        let block = Block::bordered()
            .title(title.centered())
            .title_bottom(bottom_line.centered())
            .border_set(border::THICK);

        if let Some(ref error_msg) = self.error {
            if self.stats.is_empty() {
                let error_text = vec![
                    Line::from(""),
                    Line::from("Error").red().bold().centered(),
                    Line::from(""),
                    Line::from(error_msg.as_str()).red().centered(),
                    Line::from(""),
                    Line::from(format!(
                        "Make sure the metrics server is running on http://127.0.0.1:{}",
                        self.metrics_port
                    ))
                    .yellow()
                    .centered(),
                ];

                ratatui::widgets::Paragraph::new(error_text)
                    .block(block)
                    .render(area, buf);
                return;
            }
        }

        if self.stats.is_empty() {
            let empty_text = vec![
                Line::from(""),
                Line::from("No channel statistics found")
                    .yellow()
                    .centered(),
                Line::from(""),
                Line::from("Make sure channels are instrumented and the server is running")
                    .centered(),
            ];

            ratatui::widgets::Paragraph::new(empty_text)
                .block(block)
                .render(area, buf);
            return;
        }

        // Split the area if logs are being shown
        let (table_area, logs_area) = if self.show_logs {
            let chunks = Layout::default()
                .direction(ratatui::layout::Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            (chunks[0], Some(chunks[1]))
        } else {
            (area, None)
        };

        let available_width = table_area.width.saturating_sub(10);
        let channel_width = ((available_width as f32 * 0.22) as usize).max(15);

        let header_style = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);

        let header = Row::new(vec![
            Cell::from("Channel"),
            Cell::from("Type"),
            Cell::from("State"),
            Cell::from("Sent"),
            Cell::from("Mem"),
            Cell::from("Received"),
            Cell::from("Queued"),
            Cell::from("Mem"),
            Cell::from("Usage"),
        ])
        .style(header_style)
        .height(1);

        let rows: Vec<Row> = self
            .stats
            .iter()
            .enumerate()
            .map(|(idx, stat)| {
                let (state_text, state_style) = match stat.state {
                    ChannelState::Active => {
                        (stat.state.to_string(), Style::default().fg(Color::Green))
                    }
                    ChannelState::Closed => {
                        (stat.state.to_string(), Style::default().fg(Color::Yellow))
                    }
                    ChannelState::Full => {
                        (format!("⚠ {}", stat.state), Style::default().fg(Color::Red))
                    }
                    ChannelState::Notified => {
                        (stat.state.to_string(), Style::default().fg(Color::Blue))
                    }
                };

                let mut row = Row::new(vec![
                    Cell::from(truncate_left(&stat.label, channel_width)),
                    Cell::from(stat.channel_type.to_string()),
                    Cell::from(state_text).style(state_style),
                    Cell::from(stat.sent_count.to_string()),
                    Cell::from(format_bytes(stat.total_bytes)),
                    Cell::from(stat.received_count.to_string()),
                    Cell::from(stat.queued.to_string()),
                    Cell::from(format_bytes(stat.queued_bytes)),
                    usage_bar(stat.queued, &stat.channel_type, 10),
                ]);

                // Highlight selected row
                if idx == self.selected_index {
                    row = row.style(Style::default().bg(Color::DarkGray));
                }

                row
            })
            .collect();

        let widths = [
            Constraint::Percentage(22), // Channel
            Constraint::Percentage(11), // Type
            Constraint::Percentage(9),  // State
            Constraint::Percentage(7),  // Sent
            Constraint::Percentage(9),  // Mem
            Constraint::Percentage(8),  // Received
            Constraint::Percentage(7),  // Queued
            Constraint::Percentage(9),  // Mem
            Constraint::Percentage(14), // Capacity
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .block(block)
            .column_spacing(1);

        Widget::render(table, table_area, buf);

        // Render logs panel if visible
        if let Some(logs_area) = logs_area {
            let channel_label = if self.selected_index < self.stats.len() {
                let stat = &self.stats[self.selected_index];
                if stat.label.is_empty() {
                    stat.id.clone()
                } else {
                    stat.label.clone()
                }
            } else {
                "Unknown".to_string()
            };

            if let Some(ref cached_logs) = self.logs {
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
                render_logs_panel(cached_logs, &display_label, logs_area, buf);
            } else {
                let message = if self.paused {
                    "(refresh paused)"
                } else if self.error.is_some() {
                    "(cannot fetch new data)"
                } else {
                    "(no data)"
                };
                render_logs_placeholder(&channel_label, message, logs_area, buf);
            }
        }
    }
}

fn format_delay(delay_ns: u64) -> String {
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

fn truncate_message(msg: &str, max_len: usize) -> String {
    if msg.len() <= max_len {
        format!("{:<width$}", msg, width = max_len)
    } else {
        let truncated = &msg[..max_len.saturating_sub(3)];
        format!("{}...", truncated)
    }
}

fn render_logs_placeholder(channel_label: &str, message: &str, area: Rect, buf: &mut Buffer) {
    let block = Block::bordered()
        .title(format!(" {} ", channel_label))
        .border_set(border::THICK);

    let inner_area = block.inner(area);
    block.render(area, buf);

    let message_width = message.len() as u16;
    let x = inner_area.x + (inner_area.width.saturating_sub(message_width)) / 2;
    let y = inner_area.y + inner_area.height / 2;

    if x < inner_area.x + inner_area.width && y < inner_area.y + inner_area.height {
        buf.set_string(x, y, message, Style::default().fg(Color::DarkGray));
    }
}

fn render_logs_panel(cached_logs: &CachedLogs, channel_label: &str, area: Rect, buf: &mut Buffer) {
    let block = Block::bordered()
        .title(format!(" {} ", channel_label))
        .border_set(border::THICK);

    let inner_area = block.inner(area);
    block.render(area, buf);

    let received_map = &cached_logs.received_map;

    let available_width = inner_area.width.saturating_sub(2);
    let msg_width = (available_width.saturating_sub(30) as usize).max(20);

    let header_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    let header = Row::new(vec!["Index", "Timestamp", "Message", "Delay"])
        .style(header_style)
        .height(1);

    let rows: Vec<Row> = cached_logs
        .logs
        .sent_logs
        .iter()
        .map(|entry| {
            let total_secs = entry.timestamp / 1_000_000_000;
            let millis = (entry.timestamp % 1_000_000_000) / 1_000_000;
            let minutes = (total_secs % 3600) / 60;
            let seconds = total_secs % 60;
            let timestamp = format!("{:02}:{:02}.{:03}", minutes, seconds, millis);

            let msg = entry.message.as_deref().unwrap_or("");
            let truncated_msg = truncate_message(msg, msg_width);

            let delay_str = if let Some(received_entry) = received_map.get(&entry.index) {
                if received_entry.timestamp >= entry.timestamp {
                    let delay_ns = received_entry.timestamp - entry.timestamp;
                    format_delay(delay_ns)
                } else {
                    "⚠".to_string()
                }
            } else {
                "queued".to_string()
            };

            Row::new(vec![
                entry.index.to_string(),
                timestamp,
                truncated_msg,
                delay_str,
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(6),
        Constraint::Length(13), // MM:SS.mmm format
        Constraint::Min(20),
        Constraint::Length(12),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().fg(Color::Yellow));

    Widget::render(table, inner_area, buf);
}
