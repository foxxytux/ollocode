use crate::{app::App, tui};
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode, size,
    },
};
use ratatui::layout::Rect;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{io, time::Duration};

pub async fn run(mut app: App) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    result
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        app.drain_events().await;
        terminal.draw(|frame| tui::draw(frame, app))?;

        if app.should_quit {
            break;
        }

        while event::poll(Duration::from_millis(10))? {
            match event::read()? {
                Event::Key(key) => handle_key(app, key),
                Event::Mouse(mouse) => handle_mouse(app, mouse)?,
                Event::Paste(text) => app.input_insert_str(&text),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) {
    match (key.modifiers, key.code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => app.should_quit = true,
        (KeyModifiers::CONTROL, KeyCode::Char('m')) => app.refresh_models(),
        (KeyModifiers::CONTROL, KeyCode::Char('j')) => app.select_next_model(),
        (KeyModifiers::CONTROL, KeyCode::Char('k')) => app.select_previous_model(),
        (KeyModifiers::CONTROL, KeyCode::Char('a')) => app.input_home(),
        (KeyModifiers::CONTROL, KeyCode::Char('e')) => app.input_end(),
        (_, KeyCode::Up) => app.history_previous(),
        (_, KeyCode::Down) => app.history_next(),
        (_, KeyCode::Left) => app.input_left(),
        (_, KeyCode::Right) => app.input_right(),
        (_, KeyCode::Home) => app.input_home(),
        (_, KeyCode::End) => app.input_end(),
        (_, KeyCode::Delete) => app.input_delete(),
        (_, KeyCode::PageUp) => app.scroll_transcript(8),
        (_, KeyCode::PageDown) => app.scroll_transcript(-8),
        (_, KeyCode::Enter) => app.submit_prompt(),
        (_, KeyCode::Backspace) => app.input_backspace(),
        (_, KeyCode::Char(ch)) => {
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                app.input_insert(ch);
            }
        }
        (_, KeyCode::Esc) => app.should_quit = true,
        _ => {}
    }
}

fn handle_mouse(app: &mut App, mouse: MouseEvent) -> Result<()> {
    let (width, height) = size()?;
    let areas = tui::public_areas(Rect::new(0, 0, width, height));

    match mouse.kind {
        MouseEventKind::ScrollUp => {
            if contains(areas.transcript, mouse.column, mouse.row) {
                app.scroll_transcript(3);
            } else if contains(areas.models, mouse.column, mouse.row) {
                app.select_previous_model();
            }
        }
        MouseEventKind::ScrollDown => {
            if contains(areas.transcript, mouse.column, mouse.row) {
                app.scroll_transcript(-3);
            } else if contains(areas.models, mouse.column, mouse.row) {
                app.select_next_model();
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if contains(areas.models, mouse.column, mouse.row) {
                let inner_y = mouse.row.saturating_sub(areas.models.y).saturating_sub(1);
                app.select_model_index(inner_y as usize);
            } else if contains(areas.input, mouse.column, mouse.row) {
                let inner_x = mouse.column.saturating_sub(areas.input.x).saturating_sub(1);
                let target = app
                    .input
                    .char_indices()
                    .nth(inner_x as usize)
                    .map(|(index, _)| index)
                    .unwrap_or(app.input.len());
                app.input_cursor = target;
            }
        }
        _ => {}
    }

    Ok(())
}

fn contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x
        && x < area.x.saturating_add(area.width)
        && y >= area.y
        && y < area.y.saturating_add(area.height)
}
