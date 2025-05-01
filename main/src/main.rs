use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    style::{Color, Style},
    widgets::{Block, Borders},
};
use std::io;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Terminal setup
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Draw UI
    terminal.draw(|f| {
        let size = f.area();
        let block = Block::default()
            .title(test_lib::get_message())
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::White).bg(Color::Blue));
        f.render_widget(block, size);
    })?;

    // Wait for keypress
    loop {
        if event::poll(std::time::Duration::from_millis(500))? {
            if let event::Event::Key(_) = event::read()? {
                break;
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(true, true);
    }
}
