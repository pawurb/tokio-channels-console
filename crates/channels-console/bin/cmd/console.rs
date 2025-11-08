use channels_console::{
    format_bytes, ChannelLogs, ChannelState, ChannelType, LogEntry, SerializableChannelStats,
};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use eyre::Result;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    symbols::border,
    text::{Line, Text},
    widgets::{Block, Cell, Clear, HighlightSpacing, Row, Table, TableState},
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Channels,
    Logs,
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
    table_state: TableState,
    logs_table_state: TableState,
    focus: Focus,
    show_logs: bool,
    logs: Option<CachedLogs>,
    paused: bool,
    inspect_open: bool,
    inspected_log: Option<LogEntry>,
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
            table_state: TableState::default().with_selected(0),
            logs_table_state: TableState::default(),
            focus: Focus::Channels,
            show_logs: false,
            logs: None,
            paused: false,
            inspect_open: false,
            inspected_log: None,
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

fn fetch_logs(agent: &ureq::Agent, port: u16, channel_id: u64) -> Result<ChannelLogs> {
    let url = format!("http://127.0.0.1:{}/logs/{}", port, channel_id);
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

fn usage_bar(queued: u64, channel_type: &ChannelType, _width: usize) -> Cell<'static> {
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

                if let Some(selected) = self.table_state.selected() {
                    if selected >= self.stats.len() && !self.stats.is_empty() {
                        self.table_state.select(Some(self.stats.len() - 1));
                    }
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

    fn draw(&mut self, frame: &mut Frame) {
        self.render_ui(frame);
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
            KeyCode::Char('o') | KeyCode::Char('O') => {
                if self.inspect_open {
                    self.close_inspect_and_refocus_channels();
                } else if self.focus == Focus::Logs {
                    // Close logs view when focused on a log entry
                    self.hide_logs();
                } else {
                    self.toggle_logs();
                }
            }
            KeyCode::Char('p') | KeyCode::Char('P') => self.toggle_pause(),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('H') => {
                if self.inspect_open {
                    self.close_inspect_only();
                } else {
                    self.focus_channels();
                }
            }
            KeyCode::Right | KeyCode::Char('l') => self.focus_logs(),
            KeyCode::Char('i') | KeyCode::Char('I') => self.toggle_inspect(),
            KeyCode::Up | KeyCode::Char('k') => match self.focus {
                Focus::Channels => self.select_previous(),
                Focus::Logs => self.select_previous_log(),
            },
            KeyCode::Down | KeyCode::Char('j') => match self.focus {
                Focus::Channels => self.select_next(),
                Focus::Logs => self.select_next_log(),
            },
            _ => {}
        }
    }

    fn select_previous(&mut self) {
        if !self.stats.is_empty() {
            let i = match self.table_state.selected() {
                Some(i) => i.saturating_sub(1),
                None => 0,
            };
            self.table_state.select(Some(i));

            if self.paused && self.show_logs {
                self.logs = None;
            } else if self.show_logs {
                self.refresh_logs();
            }
        }
    }

    fn select_next(&mut self) {
        if !self.stats.is_empty() {
            let i = match self.table_state.selected() {
                Some(i) => (i + 1).min(self.stats.len() - 1),
                None => 0,
            };
            self.table_state.select(Some(i));

            if self.paused && self.show_logs {
                self.logs = None;
            } else if self.show_logs {
                self.refresh_logs();
            }
        }
    }

    fn toggle_logs(&mut self) {
        let has_valid_selection = self
            .table_state
            .selected()
            .map(|i| i < self.stats.len())
            .unwrap_or(false);

        if !self.stats.is_empty() && has_valid_selection {
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
        self.logs_table_state.select(None);
        self.focus = Focus::Channels;
    }

    fn refresh_logs(&mut self) {
        if self.paused {
            return;
        }

        self.logs = None;

        if let Some(selected) = self.table_state.selected() {
            if !self.stats.is_empty() && selected < self.stats.len() {
                let channel_id = self.stats[selected].id;
                if let Ok(logs) = fetch_logs(&self.agent, self.metrics_port, channel_id) {
                    let received_map: std::collections::HashMap<u64, LogEntry> = logs
                        .received_logs
                        .iter()
                        .map(|entry| (entry.index, entry.clone()))
                        .collect();

                    self.logs = Some(CachedLogs { logs, received_map });

                    // Ensure logs table selection is valid
                    if let Some(ref cached_logs) = self.logs {
                        let log_count = cached_logs.logs.sent_logs.len();
                        if let Some(selected) = self.logs_table_state.selected() {
                            if selected >= log_count && log_count > 0 {
                                self.logs_table_state.select(Some(log_count - 1));
                            }
                        }
                    }
                }
            }
        }
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;
    }

    fn focus_channels(&mut self) {
        self.focus = Focus::Channels;
        // Clear logs table selection when not focused
        self.logs_table_state.select(None);
    }

    fn focus_logs(&mut self) {
        if self.show_logs && !self.stats.is_empty() {
            // Only allow focus if there are actual logs to display
            if let Some(ref cached_logs) = self.logs {
                if !cached_logs.logs.sent_logs.is_empty() {
                    self.focus = Focus::Logs;
                    // Ensure logs table has a valid selection
                    if self.logs_table_state.selected().is_none() {
                        self.logs_table_state.select(Some(0));
                    }
                }
            }
        }
    }

    fn select_previous_log(&mut self) {
        if let Some(ref cached_logs) = self.logs {
            let log_count = cached_logs.logs.sent_logs.len();
            if log_count > 0 {
                let i = match self.logs_table_state.selected() {
                    Some(i) => i.saturating_sub(1),
                    None => 0,
                };
                self.logs_table_state.select(Some(i));

                // Update inspected log if inspect popup is open
                if self.inspect_open {
                    if let Some(entry) = cached_logs.logs.sent_logs.get(i) {
                        self.inspected_log = Some(entry.clone());
                    }
                }
            }
        }
    }

    fn select_next_log(&mut self) {
        if let Some(ref cached_logs) = self.logs {
            let log_count = cached_logs.logs.sent_logs.len();
            if log_count > 0 {
                let i = match self.logs_table_state.selected() {
                    Some(i) => (i + 1).min(log_count - 1),
                    None => 0,
                };
                self.logs_table_state.select(Some(i));

                // Update inspected log if inspect popup is open
                if self.inspect_open {
                    if let Some(entry) = cached_logs.logs.sent_logs.get(i) {
                        self.inspected_log = Some(entry.clone());
                    }
                }
            }
        }
    }

    fn toggle_inspect(&mut self) {
        if self.focus == Focus::Logs && self.logs_table_state.selected().is_some() {
            if self.inspect_open {
                // Closing inspect popup
                self.inspect_open = false;
                self.inspected_log = None;
            } else {
                // Opening inspect popup - capture the current log entry
                if let Some(selected) = self.logs_table_state.selected() {
                    if let Some(ref cached_logs) = self.logs {
                        if let Some(entry) = cached_logs.logs.sent_logs.get(selected) {
                            self.inspected_log = Some(entry.clone());
                            self.inspect_open = true;
                        }
                    }
                }
            }
        }
    }

    fn close_inspect_and_refocus_channels(&mut self) {
        self.inspect_open = false;
        self.inspected_log = None;
        self.hide_logs();
    }

    fn close_inspect_only(&mut self) {
        self.inspect_open = false;
        self.inspected_log = None;
        self.focus = Focus::Channels;
        self.logs_table_state.select(None);
    }

    fn exit(&mut self) {
        self.exit = true;
    }
}

impl App {
    fn render_ui(&mut self, frame: &mut Frame) {
        let area = frame.area();
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

        let bottom_line = match self.focus {
            Focus::Channels => {
                if !refresh_status.is_empty() {
                    Line::from(vec![
                        " Quit ".into(),
                        "<q> ".blue().bold(),
                        " | ".into(),
                        "<↑↓←→/jkhl> ".blue().bold(),
                        " | Logs ".into(),
                        "<o> ".blue().bold(),
                        " | Pause ".into(),
                        "<p> ".blue().bold(),
                        " | ".into(),
                        refresh_status.yellow(),
                    ])
                } else {
                    Line::from(vec![
                        " Quit ".into(),
                        "<q> ".blue().bold(),
                        " | ".into(),
                        "<↑↓←→/jkhl> ".blue().bold(),
                        " | Logs ".into(),
                        "<o> ".blue().bold(),
                        " | Pause ".into(),
                        "<p> ".blue().bold(),
                    ])
                }
            }
            Focus::Logs => {
                if !refresh_status.is_empty() {
                    Line::from(vec![
                        " Quit ".into(),
                        "<q> ".blue().bold(),
                        " | ".into(),
                        "<↑↓←→/jkhl> ".blue().bold(),
                        " | Inspect ".into(),
                        "<i> ".blue().bold(),
                        " | Pause ".into(),
                        "<p> ".blue().bold(),
                        " | ".into(),
                        refresh_status.yellow(),
                    ])
                } else {
                    Line::from(vec![
                        " Quit ".into(),
                        "<q> ".blue().bold(),
                        " | ".into(),
                        "<↑↓←→/jkhl> ".blue().bold(),
                        " | Inspect ".into(),
                        "<i> ".blue().bold(),
                        " | Pause ".into(),
                        "<p> ".blue().bold(),
                    ])
                }
            }
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

                frame.render_widget(
                    ratatui::widgets::Paragraph::new(error_text).block(block),
                    area,
                );
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

            frame.render_widget(
                ratatui::widgets::Paragraph::new(empty_text).block(block),
                area,
            );
            return;
        }

        // Render the main block and get its inner area
        let inner_area = block.inner(area);
        frame.render_widget(block, area);

        // Split the inner area if logs are being shown
        let (table_area, logs_area) = if self.show_logs {
            let chunks = Layout::default()
                .direction(ratatui::layout::Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(inner_area);
            (chunks[0], Some(chunks[1]))
        } else {
            (inner_area, None)
        };

        let available_width = table_area.width.saturating_sub(10);
        let channel_width = ((available_width as f32 * 0.22) as usize).max(36);

        let header_style = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);

        let header = Row::new(vec![
            Cell::from("Channel"),
            Cell::from("Type"),
            Cell::from("State"),
            Cell::from("Sent"),
            Cell::from("Received"),
            Cell::from("Queue"),
            Cell::from("Mem"),
        ])
        .style(header_style)
        .height(1);

        let rows: Vec<Row> = self
            .stats
            .iter()
            .map(|stat| {
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

                let mem_cell = match stat.channel_type {
                    ChannelType::Unbounded => Cell::from("N/A"),
                    _ => Cell::from(format_bytes(stat.queued_bytes)),
                };

                let row = Row::new(vec![
                    Cell::from(truncate_left(&stat.label, channel_width)),
                    Cell::from(stat.channel_type.to_string()),
                    Cell::from(state_text).style(state_style),
                    Cell::from(stat.sent_count.to_string()),
                    Cell::from(stat.received_count.to_string()),
                    usage_bar(stat.queued, &stat.channel_type, 8),
                    mem_cell,
                ]);

                // Dim the row if logs are shown and channels table is not focused
                if self.show_logs && self.focus != Focus::Channels {
                    row.style(Style::default().fg(Color::DarkGray))
                } else {
                    row
                }
            })
            .collect();

        let widths = [
            Constraint::Percentage(30), // Channel
            Constraint::Percentage(14), // Type
            Constraint::Percentage(10), // State
            Constraint::Percentage(9),  // Sent
            Constraint::Percentage(11), // Received
            Constraint::Percentage(16), // Queue
            Constraint::Percentage(10), // Mem
        ];

        let selected_row_style = Style::default()
            .add_modifier(Modifier::REVERSED)
            .bg(Color::DarkGray);

        // When logs are shown, create a separate block for the channels table
        let table_block = if self.show_logs {
            let border_set = if self.focus == Focus::Channels {
                border::THICK
            } else {
                border::PLAIN
            };
            Block::bordered()
                .title(" Channels ")
                .border_set(border_set)
                .style(if self.focus == Focus::Channels {
                    Style::default()
                } else {
                    Style::default().fg(Color::DarkGray)
                })
        } else {
            Block::new()
        };

        let table = Table::new(rows, widths)
            .header(header)
            .block(table_block)
            .column_spacing(1)
            .row_highlight_style(selected_row_style)
            .highlight_symbol(Text::from(">"))
            .highlight_spacing(HighlightSpacing::Always);

        frame.render_stateful_widget(table, table_area, &mut self.table_state);

        // Render logs panel if visible
        if let Some(logs_area) = logs_area {
            let channel_label = self
                .table_state
                .selected()
                .and_then(|i| self.stats.get(i))
                .map(|stat| {
                    if stat.label.is_empty() {
                        stat.id.to_string()
                    } else {
                        stat.label.clone()
                    }
                })
                .unwrap_or_else(|| "Unknown".to_string());

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
                render_logs_panel(
                    cached_logs,
                    &display_label,
                    logs_area,
                    frame,
                    &mut self.logs_table_state,
                    self.focus == Focus::Logs,
                );
            } else {
                let message = if self.paused {
                    "(refresh paused)"
                } else if self.error.is_some() {
                    "(cannot fetch new data)"
                } else {
                    "(no data)"
                };
                render_logs_placeholder(&channel_label, message, logs_area, frame);
            }
        }

        // Render inspect popup on top of everything if open
        if self.inspect_open {
            if let Some(ref inspected_log) = self.inspected_log {
                render_inspect_popup(inspected_log, area, frame);
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

fn render_logs_placeholder(channel_label: &str, message: &str, area: Rect, frame: &mut Frame) {
    let block = Block::bordered()
        .title(format!(" {} ", channel_label))
        .border_set(border::THICK);

    let inner_area = block.inner(area);
    frame.render_widget(block, area);

    let message_width = message.len() as u16;
    let x = inner_area.x + (inner_area.width.saturating_sub(message_width)) / 2;
    let y = inner_area.y + inner_area.height / 2;

    if x < inner_area.x + inner_area.width && y < inner_area.y + inner_area.height {
        frame
            .buffer_mut()
            .set_string(x, y, message, Style::default().fg(Color::DarkGray));
    }
}

fn render_logs_panel(
    cached_logs: &CachedLogs,
    channel_label: &str,
    area: Rect,
    frame: &mut Frame,
    table_state: &mut TableState,
    is_focused: bool,
) {
    let border_set = if is_focused {
        border::THICK
    } else {
        border::PLAIN
    };

    let block = Block::bordered()
        .title(format!(" {} ", channel_label))
        .border_set(border_set)
        .style(if is_focused {
            Style::default()
        } else {
            Style::default().fg(Color::DarkGray)
        });

    let inner_area = block.inner(area);
    frame.render_widget(block, area);

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

            let row = Row::new(vec![
                entry.index.to_string(),
                timestamp,
                truncated_msg,
                delay_str,
            ]);

            // Dim the row if not focused
            if !is_focused {
                row.style(Style::default().fg(Color::DarkGray))
            } else {
                row
            }
        })
        .collect();

    let widths = [
        Constraint::Length(6),
        Constraint::Length(13), // MM:SS.mmm format
        Constraint::Min(20),
        Constraint::Length(12),
    ];

    let selected_row_style = Style::default()
        .add_modifier(Modifier::REVERSED)
        .bg(Color::DarkGray);

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(selected_row_style)
        .highlight_symbol(Text::from(">"))
        .highlight_spacing(HighlightSpacing::Always);

    frame.render_stateful_widget(table, inner_area, table_state);
}

fn render_inspect_popup(entry: &LogEntry, area: Rect, frame: &mut Frame) {
    // Center the popup at 80% of screen size
    let popup_width = (area.width as f32 * 0.8) as u16;
    let popup_height = (area.height as f32 * 0.8) as u16;
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;

    let popup_area = Rect {
        x: area.x + x,
        y: area.y + y,
        width: popup_width,
        height: popup_height,
    };

    let message = entry
        .message
        .as_deref()
        .unwrap_or("(missing \"log = true\")");

    // Clear the area to create a complete overlay
    frame.render_widget(Clear, popup_area);

    let block = Block::bordered()
        .title(format!(" Log Message (Index: {}) ", entry.index))
        .border_set(border::DOUBLE);

    let inner_area = block.inner(popup_area);

    // Render the block
    frame.render_widget(block, popup_area);

    // Wrap the message text to fit the popup width
    let text_lines: Vec<Line> = message
        .lines()
        .flat_map(|line| {
            let max_width = inner_area.width.saturating_sub(2) as usize;
            if line.len() <= max_width {
                vec![Line::from(line)]
            } else {
                // Wrap long lines
                let mut wrapped = Vec::new();
                let mut remaining = line;
                while !remaining.is_empty() {
                    let split_at = remaining
                        .char_indices()
                        .nth(max_width)
                        .map(|(i, _)| i)
                        .unwrap_or(remaining.len());
                    wrapped.push(Line::from(&remaining[..split_at]));
                    remaining = &remaining[split_at..];
                }
                wrapped
            }
        })
        .collect();

    let paragraph =
        ratatui::widgets::Paragraph::new(text_lines).wrap(ratatui::widgets::Wrap { trim: false });

    frame.render_widget(paragraph, inner_area);
}
