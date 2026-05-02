use crate::app::App;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

#[derive(Debug, Clone, Copy)]
pub struct UiAreas {
    pub transcript: Rect,
    pub models: Rect,
    pub input: Rect,
}

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let areas = layout_areas(frame.area());

    draw_header(frame, app, areas.header);
    draw_transcript(frame, app, areas.transcript);
    draw_models(frame, app, areas.models);
    draw_suggestions(frame, app, areas.suggestions);
    draw_input(frame, app, areas.input);
    draw_status(frame, app, areas.status);
}

#[derive(Debug, Clone, Copy)]
struct AllAreas {
    header: Rect,
    transcript: Rect,
    models: Rect,
    suggestions: Rect,
    input: Rect,
    status: Rect,
}

pub fn public_areas(area: Rect) -> UiAreas {
    let areas = layout_areas(area);
    UiAreas {
        transcript: areas.transcript,
        models: areas.models,
        input: areas.input,
    }
}

fn layout_areas(area: Rect) -> AllAreas {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(6),
            Constraint::Length(4),
            Constraint::Length(1),
        ])
        .split(area);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(40), Constraint::Length(28)])
        .split(chunks[1]);

    AllAreas {
        header: chunks[0],
        transcript: body[0],
        models: body[1],
        suggestions: chunks[2],
        input: chunks[3],
        status: chunks[4],
    }
}

fn draw_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let model = app.selected_model.as_deref().unwrap_or("no model selected");
    let text = vec![
        Line::from(vec![
            Span::styled("ollo-code", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(model, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::raw("cwd "),
            Span::styled(
                app.cwd.display().to_string(),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw("   /help commands  Ctrl+J/K model  Ctrl+M refresh  Ctrl+C quit"),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_transcript(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let height = area.height.saturating_sub(2) as usize;
    let lines = transcript_lines(app);
    let visible = visible_tail(&lines, height, app.transcript_scroll);

    let title = if app.transcript_scroll == 0 {
        "Transcript"
    } else {
        "Transcript (scrolled)"
    };
    frame.render_widget(
        Paragraph::new(visible).block(Block::default().title(title).borders(Borders::ALL)),
        area,
    );
}

fn draw_input(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let title = if app.busy { "Prompt (busy)" } else { "Prompt" };
    let inner_width = area.width.saturating_sub(2) as usize;
    let cursor_chars = app.input[..app.input_cursor].chars().count();
    let start_chars = cursor_chars.saturating_sub(inner_width.saturating_sub(1));
    let visible: String = app
        .input
        .chars()
        .skip(start_chars)
        .take(inner_width)
        .collect();
    let cursor_x = (cursor_chars - start_chars).min(inner_width.saturating_sub(1)) as u16;

    frame.render_widget(
        Paragraph::new(visible)
            .block(Block::default().title(title).borders(Borders::ALL))
            .style(Style::default().fg(Color::White)),
        area,
    );
    frame.set_cursor_position(Position::new(
        area.x.saturating_add(1).saturating_add(cursor_x),
        area.y.saturating_add(1),
    ));
}

fn draw_suggestions(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let suggestions = app.command_suggestions();
    let lines = if suggestions.is_empty() {
        vec![Line::from(vec![
            Span::styled("Type / for commands", Style::default().fg(Color::DarkGray)),
            Span::raw("   "),
            Span::styled(
                "Paste works with bracketed paste",
                Style::default().fg(Color::DarkGray),
            ),
        ])]
    } else {
        suggestions
            .iter()
            .take(area.height.saturating_sub(2) as usize)
            .map(|command| {
                Line::from(vec![
                    Span::styled(command.usage, Style::default().fg(Color::Cyan)),
                    Span::raw("  "),
                    Span::styled(command.description, Style::default().fg(Color::Gray)),
                ])
            })
            .collect()
    };

    frame.render_widget(
        Paragraph::new(lines).block(Block::default().title("Commands").borders(Borders::ALL)),
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

fn transcript_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for item in &app.transcript {
        let style = match item.role.as_str() {
            "user" => Style::default().fg(Color::Green),
            "assistant" => Style::default().fg(Color::White),
            "tool" => Style::default().fg(Color::Yellow),
            _ => Style::default(),
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", item.role),
                style.add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                item.timestamp.format("%H:%M:%S").to_string(),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        let content = if item.content.trim().is_empty() {
            "..."
        } else {
            item.content.trim_end()
        };
        for line in content.lines() {
            lines.push(Line::from(line.to_string()));
        }
        lines.push(Line::from(""));
    }
    lines
}

fn visible_tail(lines: &[Line<'static>], height: usize, scroll: usize) -> Vec<Line<'static>> {
    if lines.len() <= height {
        return lines.to_vec();
    }
    let end = lines.len().saturating_sub(scroll).max(height);
    let start = end.saturating_sub(height);
    lines[start..end].to_vec()
}
