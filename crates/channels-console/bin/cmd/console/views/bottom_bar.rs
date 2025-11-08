use ratatui::{
    layout::Rect,
    style::Stylize,
    symbols::border,
    text::Line,
    widgets::{Block, Paragraph},
    Frame,
};

use crate::cmd::console::state::Focus;

/// Renders the bottom controls bar showing context-aware keybindings
pub fn render_bottom_bar(frame: &mut Frame, area: Rect, focus: Focus) {
    let controls_line = match focus {
        Focus::Channels => Line::from(vec![
            " Quit ".into(),
            "<q> ".blue().bold(),
            " | Navigate ".into(),
            "<←↑↓→/hjkl> ".blue().bold(),
            " | Toggle Logs ".into(),
            "<o> ".blue().bold(),
            " | Pause ".into(),
            "<p> ".blue().bold(),
        ]),
        Focus::Logs => Line::from(vec![
            " Quit ".into(),
            "<q> ".blue().bold(),
            " | Navigate ".into(),
            "<←↑↓→/hjkl> ".blue().bold(),
            " | Toggle Logs ".into(),
            "<o> ".blue().bold(),
            " | Pause ".into(),
            "<p> ".blue().bold(),
            " | Inspect ".into(),
            "<i> ".blue().bold(),
        ]),
        Focus::Inspect => Line::from(vec![
            " Quit ".into(),
            "<q> ".blue().bold(),
            " | Navigate ".into(),
            "<←↑↓→/hjkl> ".blue().bold(),
            " | Toggle Logs ".into(),
            "<o> ".blue().bold(),
            " | Pause ".into(),
            "<p> ".blue().bold(),
            " | Close ".into(),
            "<i/o/h> ".blue().bold(),
        ]),
    };

    let block = Block::bordered()
        .title(" Controls ")
        .border_set(border::PLAIN);

    let paragraph = Paragraph::new(controls_line).block(block).left_aligned();

    frame.render_widget(paragraph, area);
}
