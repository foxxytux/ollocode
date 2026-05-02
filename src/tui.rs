use crate::app::App;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(4),
            Constraint::Length(1),
        ])
        .split(area);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(40), Constraint::Length(28)])
        .split(chunks[1]);

    draw_header(frame, app, chunks[0]);
    draw_transcript(frame, app, body[0]);
    draw_models(frame, app, body[1]);
    draw_input(frame, app, chunks[2]);
    draw_status(frame, app, chunks[3]);
}

fn draw_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let model = app.selected_model.as_deref().unwrap_or("no model selected");
    let text = vec![
        Line::from(vec![
            Span::styled("Ollo Code", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(model, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::raw("cwd "),
            Span::styled(
                app.cwd.display().to_string(),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw("   Ctrl+J/K model  Ctrl+M refresh  Ctrl+C quit"),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_transcript(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let height = area.height.saturating_sub(2) as usize;
    let mut items = Vec::new();

    for item in app.transcript.iter().rev().take(height).rev() {
        let style = match item.role.as_str() {
            "user" => Style::default().fg(Color::Green),
            "assistant" => Style::default().fg(Color::White),
            "tool" => Style::default().fg(Color::Yellow),
            _ => Style::default(),
        };
        let preview = if item.content.trim().is_empty() {
            "..."
        } else {
            item.content.trim_end()
        };
        items.push(ListItem::new(vec![
            Line::from(vec![
                Span::styled(
                    format!("{} ", item.role),
                    style.add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    item.timestamp.format("%H:%M:%S").to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            Line::from(preview.to_string()),
            Line::from(""),
        ]));
    }

    frame.render_widget(
        List::new(items).block(Block::default().title("Transcript").borders(Borders::ALL)),
        area,
    );
}

fn draw_input(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let title = if app.busy { "Prompt (busy)" } else { "Prompt" };
    frame.render_widget(
        Paragraph::new(app.input.as_str())
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_models(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let selected = app.selected_model.as_deref();
    let items = app.models.iter().map(|model| {
        let is_selected = selected == Some(model.name.as_str());
        let marker = if is_selected { "> " } else { "  " };
        let style = if is_selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        ListItem::new(Line::from(vec![
            Span::styled(marker, style),
            Span::styled(model.name.clone(), style),
        ]))
    });

    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title("Ollama Models")
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn draw_status(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let style = if app.status.to_lowercase().contains("error") {
        Style::default().fg(Color::Red)
    } else if app.busy {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    frame.render_widget(Paragraph::new(app.status.as_str()).style(style), area);
}
