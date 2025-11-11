use channels_console::{LogEntry, SerializableChannelStats};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use eyre::Result;
use ratatui::{
    layout::{Constraint, Layout},
    widgets::TableState,
    DefaultTerminal, Frame,
};
use std::io;
use std::time::{Duration, Instant};

use super::http::{fetch_logs, fetch_metrics};
use super::state::{CachedLogs, Focus};
use super::views::bottom_bar::render_bottom_bar;
use super::views::main_view::render_main_view;
use super::views::top_bar::render_top_bar;

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
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_millis(2000)))
            .timeout_recv_body(Some(Duration::from_millis(1500)))
            .build();

        let agent: ureq::Agent = config.into();

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
        let refresh_interval = std::env::var("CHANNELS_CONSOLE_TUI_REFRESH_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(200));

        self.refresh_data();

        while !self.exit {
            if !self.paused && self.last_refresh.elapsed() >= refresh_interval {
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
        let selected_channel_id = self
            .table_state
            .selected()
            .and_then(|idx| self.stats.get(idx))
            .map(|stat| stat.id);

        match fetch_metrics(&self.agent, self.metrics_port) {
            Ok(stats) => {
                self.stats = stats;
                self.error = None;
                self.last_successful_fetch = Some(Instant::now());

                // Try to restore selection to the same channel ID
                if let Some(channel_id) = selected_channel_id {
                    // Find the new index of the previously selected channel
                    if let Some(new_idx) = self.stats.iter().position(|stat| stat.id == channel_id)
                    {
                        self.table_state.select(Some(new_idx));
                    } else {
                        // Channel no longer exists, select the last one if available
                        if !self.stats.is_empty() {
                            self.table_state.select(Some(self.stats.len() - 1));
                        }
                    }
                } else if let Some(selected) = self.table_state.selected() {
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
        if !self.show_logs {
            self.toggle_logs();
        } else if !self.stats.is_empty() {
            if let Some(ref cached_logs) = self.logs {
                if !cached_logs.logs.sent_logs.is_empty() {
                    self.focus = Focus::Logs;
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

        // Create 3-row vertical layout: top bar, main view, bottom bar
        let chunks = Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Top bar
                Constraint::Min(0),    // Main view (fills remaining space)
                Constraint::Length(3), // Bottom bar
            ])
            .split(area);

        // Render top status bar
        render_top_bar(
            frame,
            chunks[0],
            self.paused,
            self.last_successful_fetch,
            self.error.is_some(),
            !self.stats.is_empty(),
        );

        // Render main content area
        render_main_view(
            frame,
            chunks[1],
            &self.stats,
            &self.error,
            self.metrics_port,
            &mut self.table_state,
            &mut self.logs_table_state,
            self.focus,
            self.show_logs,
            &self.logs,
            self.paused,
            &self.inspected_log,
        );

        render_bottom_bar(frame, chunks[2], self.focus, self.last_render_duration);
    }
}
