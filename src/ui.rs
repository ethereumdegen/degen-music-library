use crate::app::{App, CredentialField, Screen};
use crate::storage::EntryKind;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph, Wrap};

const ACCENT: Color = Color::Rgb(122, 162, 247);
const SECONDARY: Color = Color::Rgb(187, 154, 247);
const SUCCESS: Color = Color::Rgb(158, 206, 106);
const MUTED: Color = Color::Rgb(86, 95, 137);
const ERROR: Color = Color::Rgb(247, 118, 142);
const EQUALIZER: [&str; 5] = ["[=    ]", "[==   ]", "[ === ]", "[  ===]", "[   ==]"];

pub fn draw(frame: &mut ratatui::Frame<'_>, app: &App) {
  match app.screen {
    Screen::Credentials => draw_credentials(frame, app),
    Screen::Browser => draw_browser(frame, app),
  }
}

fn draw_credentials(frame: &mut ratatui::Frame<'_>, app: &App) {
  let area = centered_rect(78, 25, frame.area());
  frame.render_widget(
    Block::default()
      .borders(Borders::ALL)
      .border_style(Style::default().fg(ACCENT))
      .title(Line::from(Span::styled(
        " DEGEN MUSIC LIBRARY ",
        Style::default().fg(SECONDARY).add_modifier(Modifier::BOLD),
      ))),
    area,
  );

  let inner = Rect {
    x: area.x.saturating_add(2),
    y: area.y.saturating_add(2),
    width: area.width.saturating_sub(4),
    height: area.height.saturating_sub(4),
  };
  let rows = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
      Constraint::Length(2),
      Constraint::Length(3),
      Constraint::Length(3),
      Constraint::Length(3),
      Constraint::Length(3),
      Constraint::Length(3),
      Constraint::Length(2),
      Constraint::Min(1),
    ])
    .split(inner);

  frame.render_widget(
    Paragraph::new("Connect directly to Cloudflare R2, MinIO, or any S3-compatible bucket.")
      .style(Style::default().fg(Color::White)),
    rows[0],
  );

  for (index, field) in CredentialField::ALL.iter().copied().enumerate() {
    let focused = app.credentials.selected == index;
    let value = if field == CredentialField::SecretKey {
      "*".repeat(app.credentials.value(field).chars().count())
    } else {
      app.credentials.value(field).to_owned()
    };
    let shown = if focused && !app.busy {
      format!("{value}_")
    } else {
      value
    };
    let style = if focused {
      Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
      Style::default().fg(MUTED)
    };
    frame.render_widget(
      Paragraph::new(shown).block(
        Block::default()
          .borders(Borders::ALL)
          .border_style(style)
          .title(field.label()),
      ),
      rows[index + 1],
    );
  }

  frame.render_widget(
    Paragraph::new("R2 endpoint: https://<account-id>.r2.cloudflarestorage.com   Region: auto")
      .style(Style::default().fg(MUTED)),
    rows[6],
  );
  let status_style = if app.status.starts_with("Connection failed") {
    Style::default().fg(ERROR)
  } else if app.busy {
    Style::default().fg(SECONDARY)
  } else {
    Style::default().fg(SUCCESS)
  };
  frame.render_widget(
    Paragraph::new(vec![
      Line::styled(&app.status, status_style),
      Line::from("Tab/Up/Down: field   Enter: connect   Ctrl+U: clear field   Esc: quit"),
      Line::styled(
        "Credentials are never written to disk.",
        Style::default().fg(MUTED),
      ),
    ])
    .alignment(Alignment::Center)
    .wrap(Wrap { trim: true }),
    rows[7],
  );
}

fn draw_browser(frame: &mut ratatui::Frame<'_>, app: &App) {
  let rows = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
      Constraint::Length(3),
      Constraint::Min(8),
      Constraint::Length(5),
      Constraint::Length(3),
    ])
    .split(frame.area());

  let location = if app.prefix.is_empty() {
    format!("s3://{}/", app.bucket())
  } else {
    format!("s3://{}/{}", app.bucket(), app.prefix)
  };
  frame.render_widget(
    Paragraph::new(location)
      .style(Style::default().fg(SECONDARY).add_modifier(Modifier::BOLD))
      .block(
        Block::default()
          .borders(Borders::ALL)
          .border_style(Style::default().fg(ACCENT))
          .title("Library"),
      ),
    rows[0],
  );

  let items = app
    .entries
    .iter()
    .map(|entry| {
      let playing = app
        .now_playing
        .as_ref()
        .is_some_and(|track| track.key == entry.key);
      let marker = if playing { ">" } else { " " };
      let text = match entry.kind {
        EntryKind::Folder => format!("{marker} [DIR] {}/", entry.name),
        EntryKind::Track => format!(
          "{marker} [AUDIO] {:<48} {:>9}",
          entry.name,
          human_size(entry.size)
        ),
      };
      let style = if playing {
        Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD)
      } else if entry.kind == EntryKind::Folder {
        Style::default().fg(ACCENT)
      } else {
        Style::default()
      };
      ListItem::new(Line::styled(text, style))
    })
    .collect::<Vec<_>>();

  let list_title = if app.busy {
    "Bucket Browser - loading..."
  } else {
    "Bucket Browser - Enter: open/play  Backspace: parent"
  };
  let list = List::new(items)
    .block(Block::default().borders(Borders::ALL).title(list_title))
    .highlight_symbol("-> ")
    .highlight_style(
      Style::default()
        .bg(Color::Rgb(41, 46, 66))
        .fg(Color::White)
        .add_modifier(Modifier::BOLD),
    );
  let mut list_state =
    ListState::default().with_selected((!app.entries.is_empty()).then_some(app.selected));
  frame.render_stateful_widget(list, rows[1], &mut list_state);

  draw_now_playing(frame, rows[2], app);

  let status_style = if app.status.starts_with("Could not") || app.status.starts_with("Connection")
  {
    Style::default().fg(ERROR)
  } else {
    Style::default().fg(Color::White)
  };
  frame.render_widget(
    Paragraph::new(Line::from(vec![
      Span::styled(format!(" {} ", app.status), status_style),
      Span::styled(
        " | Space pause | N/P next/previous | +/- volume | X stop | R reload | C connect | Q quit ",
        Style::default().fg(MUTED),
      ),
    ]))
    .block(Block::default().borders(Borders::ALL).title("Controls")),
    rows[3],
  );
}

fn draw_now_playing(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
  let outer = Block::default().borders(Borders::ALL).title("Now Playing");
  let inner = outer.inner(area);
  frame.render_widget(outer, area);
  let columns = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Min(20), Constraint::Length(13)])
    .split(inner);

  if let Some(track) = app.now_playing.as_ref() {
    let position = app.position();
    let duration = track.duration;
    let label = match duration {
      Some(duration) => format!(
        "{}  {} / {}",
        track.name,
        format_duration(position),
        format_duration(duration)
      ),
      None => format!("{}  {}", track.name, format_duration(position)),
    };
    let ratio = duration
      .filter(|duration| !duration.is_zero())
      .map_or(0.0, |duration| {
        position.as_secs_f64() / duration.as_secs_f64()
      })
      .clamp(0.0, 1.0);
    frame.render_widget(
      Gauge::default()
        .gauge_style(Style::default().fg(ACCENT))
        .label(label)
        .ratio(ratio),
      columns[0],
    );
    let activity = if app.busy {
      "LOADING"
    } else if app.completed {
      "FINISHED"
    } else if app.paused {
      "PAUSED"
    } else {
      EQUALIZER[app.animation_frame]
    };
    frame.render_widget(
      Paragraph::new(format!("{activity}\nVol {}%", app.volume))
        .alignment(Alignment::Right)
        .style(Style::default().fg(SUCCESS)),
      columns[1],
    );
  } else {
    frame.render_widget(
      Paragraph::new(format!(
        "Nothing playing                                      Volume {}%",
        app.volume
      ))
      .style(Style::default().fg(MUTED)),
      inner,
    );
  }
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
  let vertical = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
      Constraint::Fill(1),
      Constraint::Length(height.min(area.height)),
      Constraint::Fill(1),
    ])
    .split(area);
  Layout::default()
    .direction(Direction::Horizontal)
    .constraints([
      Constraint::Percentage((100 - percent_x) / 2),
      Constraint::Percentage(percent_x),
      Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vertical[1])[1]
}

fn human_size(bytes: i64) -> String {
  let bytes = bytes.max(0) as f64;
  const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
  let mut size = bytes;
  let mut unit = 0;
  while size >= 1024.0 && unit < UNITS.len() - 1 {
    size /= 1024.0;
    unit += 1;
  }
  if unit == 0 {
    format!("{} {}", size as u64, UNITS[unit])
  } else {
    format!("{size:.1} {}", UNITS[unit])
  }
}

fn format_duration(duration: std::time::Duration) -> String {
  let seconds = duration.as_secs();
  format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn sizes_and_durations_are_human_readable() {
    assert_eq!(human_size(512), "512 B");
    assert_eq!(human_size(1_572_864), "1.5 MiB");
    assert_eq!(format_duration(std::time::Duration::from_secs(125)), "2:05");
  }
}
