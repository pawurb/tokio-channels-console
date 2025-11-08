use channels_console::{LogEntry, SerializableChannelStats};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use eyre::Result;
use ratatui::{
    layout::{Constraint, Layout},
    style::Stylize,
    symbols::border,
    text::Line,
    widgets::{Block, TableState},
    DefaultTerminal, Frame,
};
use std::io;
use std::time::{Duration, Instant};

use super::http::{fetch_logs, fetch_metrics};
use super::state::{CachedLogs, Focus};
use super::views::channels::render_channels_panel;
use super::views::inspect::render_inspect_popup;
use super::views::logs::{render_logs_panel, render_logs_placeholder};

#[derive(Debug, Parser)]
pub struct ConsoleArgs {
    /// Port for the metrics server
    #[arg(long, default_value = "6770")]
    pub metrics_port: u16,
}

pub(crate) struct App {
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
            inspected_log: None,
            agent,
        };

        let mut terminal = ratatui::init();
        let app_result = app.run(&mut terminal);
        ratatui::restore();
        app_result.map_err(|e| eyre::eyre!("TUI error: {}", e))
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
            KeyCode::Char('o') | KeyCode::Char('O') => match self.focus {
                Focus::Inspect => self.close_inspect_and_refocus_channels(),
                Focus::Logs => self.hide_logs(),
                Focus::Channels => self.toggle_logs(),
            },
            KeyCode::Char('p') | KeyCode::Char('P') => self.toggle_pause(),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('H') => {
                if self.focus == Focus::Inspect {
                    self.close_inspect_only();
                } else {
                    self.focus_channels();
                }
            }
            KeyCode::Right | KeyCode::Char('l') => self.focus_logs(),
            KeyCode::Char('i') | KeyCode::Char('I') => self.toggle_inspect(),
            KeyCode::Up | KeyCode::Char('k') => match self.focus {
                Focus::Channels => self.select_previous_channel(),
                Focus::Logs | Focus::Inspect => self.select_previous_log(),
            },
            KeyCode::Down | KeyCode::Char('j') => match self.focus {
                Focus::Channels => self.select_next_channel(),
                Focus::Logs | Focus::Inspect => self.select_next_log(),
            },
            _ => {}
        }
    }

    fn select_previous_channel(&mut self) {
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

    fn select_next_channel(&mut self) {
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
                if self.focus == Focus::Inspect {
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
                if self.focus == Focus::Inspect {
                    if let Some(entry) = cached_logs.logs.sent_logs.get(i) {
                        self.inspected_log = Some(entry.clone());
                    }
                }
            }
        }
    }

    fn toggle_inspect(&mut self) {
        if self.focus == Focus::Inspect {
            // Closing inspect popup
            self.focus = Focus::Logs;
            self.inspected_log = None;
        } else if self.focus == Focus::Logs && self.logs_table_state.selected().is_some() {
            // Opening inspect popup - capture the current log entry
            if let Some(selected) = self.logs_table_state.selected() {
                if let Some(ref cached_logs) = self.logs {
                    if let Some(entry) = cached_logs.logs.sent_logs.get(selected) {
                        self.inspected_log = Some(entry.clone());
                        self.focus = Focus::Inspect;
                    }
                }
            }
        }
    }

    fn close_inspect_and_refocus_channels(&mut self) {
        self.inspected_log = None;
        self.hide_logs();
    }

    fn close_inspect_only(&mut self) {
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
            Focus::Inspect => {
                if !refresh_status.is_empty() {
                    Line::from(vec![
                        " Quit ".into(),
                        "<q> ".blue().bold(),
                        " | ".into(),
                        "<↑↓/jk> ".blue().bold(),
                        " | Close ".into(),
                        "<i/o/h> ".blue().bold(),
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
                        "<↑↓/jk> ".blue().bold(),
                        " | Close ".into(),
                        "<i/o/h> ".blue().bold(),
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

        render_channels_panel(
            &self.stats,
            table_area,
            frame,
            &mut self.table_state,
            self.show_logs,
            self.focus,
        );

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
        if self.focus == Focus::Inspect {
            if let Some(ref inspected_log) = self.inspected_log {
                render_inspect_popup(inspected_log, area, frame);
            }
        }
    }
}
