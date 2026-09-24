use crate::player::{self, AudioPlayer, CachedTrack, PreparedTrack};
use crate::storage::{ConnectionDetails, Entry, EntryKind, Storage};
use crate::ui;
use anyhow::{Context, Result};
use crossterm::event::{
  self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
  disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, Stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use zeroize::Zeroize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
  Credentials,
  Browser,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RepeatMode {
  #[default]
  Off,
  Once,
  Forever,
}

impl RepeatMode {
  pub fn label(self) -> &'static str {
    match self {
      Self::Off => "Off",
      Self::Once => "Once",
      Self::Forever => "Forever",
    }
  }

  fn cycle(self) -> Self {
    match self {
      Self::Off => Self::Once,
      Self::Once => Self::Forever,
      Self::Forever => Self::Off,
    }
  }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompletionAction {
  Advance,
  Replay,
}

fn completion_action(mode: &mut RepeatMode) -> CompletionAction {
  match mode {
    RepeatMode::Off => CompletionAction::Advance,
    RepeatMode::Once => {
      *mode = RepeatMode::Off;
      CompletionAction::Replay
    }
    RepeatMode::Forever => CompletionAction::Replay,
  }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialField {
  Endpoint,
  Region,
  Bucket,
  AccessKey,
  SecretKey,
}

impl CredentialField {
  pub const ALL: [Self; 5] = [
    Self::Endpoint,
    Self::Region,
    Self::Bucket,
    Self::AccessKey,
    Self::SecretKey,
  ];

  pub fn label(self) -> &'static str {
    match self {
      Self::Endpoint => "S3 endpoint",
      Self::Region => "Region",
      Self::Bucket => "Bucket",
      Self::AccessKey => "Access key ID",
      Self::SecretKey => "Secret access key",
    }
  }
}

pub struct CredentialForm {
  pub endpoint: String,
  pub region: String,
  pub bucket: String,
  pub access_key: String,
  pub secret_key: String,
  pub selected: usize,
}

impl Default for CredentialForm {
  fn default() -> Self {
    Self {
      endpoint: String::new(),
      region: "auto".to_owned(),
      bucket: String::new(),
      access_key: String::new(),
      secret_key: String::new(),
      selected: 0,
    }
  }
}

impl CredentialForm {
  pub fn value(&self, field: CredentialField) -> &str {
    match field {
      CredentialField::Endpoint => &self.endpoint,
      CredentialField::Region => &self.region,
      CredentialField::Bucket => &self.bucket,
      CredentialField::AccessKey => &self.access_key,
      CredentialField::SecretKey => &self.secret_key,
    }
  }

  fn value_mut(&mut self, field: CredentialField) -> &mut String {
    match field {
      CredentialField::Endpoint => &mut self.endpoint,
      CredentialField::Region => &mut self.region,
      CredentialField::Bucket => &mut self.bucket,
      CredentialField::AccessKey => &mut self.access_key,
      CredentialField::SecretKey => &mut self.secret_key,
    }
  }

  fn details(&self) -> ConnectionDetails {
    ConnectionDetails {
      endpoint: self.endpoint.clone(),
      region: self.region.clone(),
      bucket: self.bucket.clone(),
      access_key: self.access_key.clone(),
      secret_key: self.secret_key.clone(),
    }
  }

  fn clear_sensitive(&mut self) {
    self.access_key.zeroize();
    self.secret_key.zeroize();
  }
}

pub struct CurrentTrack {
  pub key: String,
  pub name: String,
  pub duration: Option<Duration>,
  cache: Arc<CachedTrack>,
}

pub struct App {
  pub screen: Screen,
  pub credentials: CredentialForm,
  pub entries: Vec<Entry>,
  pub selected: usize,
  pub prefix: String,
  pub status: String,
  pub busy: bool,
  pub volume: u8,
  pub paused: bool,
  pub completed: bool,
  pub now_playing: Option<CurrentTrack>,
  pub animation_frame: usize,
  pub repeat_mode: RepeatMode,
  pub show_visualizer: bool,
  pub spectrum: [f32; crate::visualizer::BAR_COUNT],
  storage: Option<Storage>,
  player: Option<AudioPlayer>,
  generation: u64,
  finished_handled: bool,
  track_task: Option<JoinHandle<()>>,
  pending_track: Option<usize>,
}

impl Default for App {
  fn default() -> Self {
    Self {
      screen: Screen::Credentials,
      credentials: CredentialForm::default(),
      entries: Vec::new(),
      selected: 0,
      prefix: String::new(),
      status: "Enter your S3-compatible bucket credentials. They are kept in memory only."
        .to_owned(),
      busy: false,
      volume: 80,
      paused: false,
      completed: false,
      now_playing: None,
      animation_frame: 0,
      repeat_mode: RepeatMode::Off,
      show_visualizer: false,
      spectrum: [0.0; crate::visualizer::BAR_COUNT],
      storage: None,
      player: None,
      generation: 0,
      finished_handled: true,
      track_task: None,
      pending_track: None,
    }
  }
}

impl App {
  pub fn bucket(&self) -> &str {
    self.storage.as_ref().map_or("", Storage::bucket)
  }

  pub fn position(&self) -> Duration {
    self
      .player
      .as_ref()
      .map_or(Duration::ZERO, AudioPlayer::position)
  }

  fn next_generation(&mut self) -> u64 {
    self.generation = self.generation.wrapping_add(1);
    self.generation
  }

  fn begin_connect(&mut self, tx: &mpsc::UnboundedSender<AppEvent>) {
    if self.busy {
      return;
    }
    let details = self.credentials.details();
    let generation = self.next_generation();
    self.busy = true;
    self.status = "Connecting and reading the bucket root...".to_owned();
    let tx = tx.clone();
    tokio::spawn(async move {
      let result = async {
        let storage = Storage::connect(details)?;
        let entries = storage.list("").await?;
        Ok::<_, anyhow::Error>((storage, entries))
      }
      .await
      .map_err(|error| format!("Connection failed: {error:#}"));
      let _ = tx.send(AppEvent::Connected { generation, result });
    });
  }

  fn begin_list(&mut self, prefix: String, tx: &mpsc::UnboundedSender<AppEvent>) {
    if self.busy {
      return;
    }
    let Some(storage) = self.storage.clone() else {
      return;
    };
    let generation = self.next_generation();
    self.busy = true;
    self.status = format!("Loading /{prefix}...");
    let tx = tx.clone();
    tokio::spawn(async move {
      let result = storage
        .list(&prefix)
        .await
        .map_err(|error| format!("Could not list folder: {error:#}"));
      let _ = tx.send(AppEvent::Listed {
        generation,
        prefix,
        result,
      });
    });
  }

  fn begin_play(&mut self, index: usize, tx: &mpsc::UnboundedSender<AppEvent>) {
    let Some(entry) = self.entries.get(index).cloned() else {
      return;
    };
    if entry.kind != EntryKind::Track {
      return;
    }
    let Some(storage) = self.storage.clone() else {
      return;
    };

    if let Some(task) = self.track_task.take() {
      task.abort();
    }
    if let Some(player) = self.player.as_ref() {
      player.stop();
    }
    self.now_playing = None;
    self.paused = false;
    self.completed = false;
    self.finished_handled = true;
    self.spectrum = [0.0; crate::visualizer::BAR_COUNT];
    self.selected = index;
    self.pending_track = Some(index);

    let generation = self.next_generation();
    self.busy = true;
    self.status = format!("Loading {}...", entry.name);
    let tx = tx.clone();
    self.track_task = Some(tokio::spawn(async move {
      let result = async {
        let downloaded = storage.download(&entry.key).await?;
        tokio::task::spawn_blocking(move || player::prepare(downloaded))
          .await
          .context("audio decoder task failed")?
      }
      .await
      .map_err(|error| format!("Could not play {}: {error:#}", entry.name));
      let _ = tx.send(AppEvent::TrackReady {
        generation,
        index,
        key: entry.key,
        name: entry.name,
        result,
      });
    }));
  }

  fn begin_replay(&mut self, tx: &mpsc::UnboundedSender<AppEvent>) {
    if self.busy {
      return;
    }
    let Some(track) = self.now_playing.as_ref() else {
      return;
    };
    let cache = Arc::clone(&track.cache);
    let key = track.key.clone();
    let name = track.name.clone();
    let index = self
      .entries
      .iter()
      .position(|entry| entry.key == key)
      .unwrap_or(self.selected);
    if let Some(task) = self.track_task.take() {
      task.abort();
    }
    let generation = self.next_generation();
    self.busy = true;
    self.pending_track = Some(index);
    self.status = format!("Looping {name}...");
    let tx = tx.clone();
    self.track_task = Some(tokio::spawn(async move {
      let result = tokio::task::spawn_blocking(move || player::prepare_cached(cache))
        .await
        .context("audio decoder task failed")
        .and_then(|result| result)
        .map_err(|error| format!("Could not replay {name}: {error:#}"));
      let _ = tx.send(AppEvent::TrackReady {
        generation,
        index,
        key,
        name,
        result,
      });
    }));
  }

  fn activate_selected(&mut self, tx: &mpsc::UnboundedSender<AppEvent>) {
    let Some(entry) = self.entries.get(self.selected) else {
      return;
    };
    match entry.kind {
      EntryKind::Folder => self.begin_list(entry.key.clone(), tx),
      EntryKind::Track => self.begin_play(self.selected, tx),
    }
  }

  fn go_to_parent(&mut self, tx: &mpsc::UnboundedSender<AppEvent>) {
    if self.prefix.is_empty() {
      return;
    }
    self.begin_list(parent_prefix(&self.prefix), tx);
  }

  fn change_selection(&mut self, amount: isize) {
    if self.entries.is_empty() {
      self.selected = 0;
      return;
    }
    self.selected = self
      .selected
      .saturating_add_signed(amount)
      .min(self.entries.len() - 1);
  }

  fn change_volume(&mut self, amount: i16) {
    self.volume = (i16::from(self.volume) + amount).clamp(0, 100) as u8;
    if let Some(player) = self.player.as_mut() {
      player.set_volume(self.volume);
    }
    self.status = format!("Volume {}%", self.volume);
  }

  fn cycle_repeat_mode(&mut self) {
    self.repeat_mode = self.repeat_mode.cycle();
    self.status = match self.repeat_mode {
      RepeatMode::Off => "Loop mode: off.".to_owned(),
      RepeatMode::Once => "Loop mode: replay the current track once.".to_owned(),
      RepeatMode::Forever => "Loop mode: replay the current track forever.".to_owned(),
    };
  }

  fn toggle_visualizer(&mut self) {
    self.show_visualizer = !self.show_visualizer;
    self.status = if self.show_visualizer {
      "FFT visualizer enabled. Press V to return to the library.".to_owned()
    } else {
      "Library view enabled. Press V for the FFT visualizer.".to_owned()
    };
  }

  fn toggle_pause(&mut self) {
    let Some(player) = self.player.as_ref() else {
      return;
    };
    if self.now_playing.is_none() || self.completed {
      return;
    }
    self.paused = player.toggle_pause();
    self.status = if self.paused { "Paused" } else { "Playing" }.to_owned();
  }

  fn stop(&mut self) {
    if let Some(task) = self.track_task.take() {
      task.abort();
      self.next_generation();
    }
    self.pending_track = None;
    self.busy = false;
    if let Some(player) = self.player.as_ref() {
      player.stop();
    }
    self.now_playing = None;
    self.paused = false;
    self.completed = false;
    self.finished_handled = true;
    self.spectrum = [0.0; crate::visualizer::BAR_COUNT];
    self.status = "Stopped".to_owned();
  }

  fn adjacent_track(&self, forward: bool) -> Option<usize> {
    let current = self.pending_track.unwrap_or_else(|| {
      self
        .now_playing
        .as_ref()
        .and_then(|track| self.entries.iter().position(|entry| entry.key == track.key))
        .unwrap_or(self.selected)
    });
    if forward {
      self
        .entries
        .iter()
        .enumerate()
        .skip(current.saturating_add(1))
        .find_map(|(index, entry)| (entry.kind == EntryKind::Track).then_some(index))
    } else {
      self.entries[..current.min(self.entries.len())]
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, entry)| (entry.kind == EntryKind::Track).then_some(index))
    }
  }

  fn play_adjacent(&mut self, forward: bool, tx: &mpsc::UnboundedSender<AppEvent>) {
    if let Some(index) = self.adjacent_track(forward) {
      self.selected = index;
      self.begin_play(index, tx);
    } else {
      self.status = if forward {
        "No next track in this folder."
      } else {
        "No previous track in this folder."
      }
      .to_owned();
    }
  }

  fn open_credentials(&mut self) {
    self.stop();
    self.next_generation();
    self.busy = false;
    self.storage = None;
    self.entries.clear();
    self.prefix.clear();
    self.selected = 0;
    self.screen = Screen::Credentials;
    self.show_visualizer = false;
    self.status = "Enter credentials for an S3-compatible bucket.".to_owned();
  }

  fn handle_credentials_key(
    &mut self,
    key: KeyEvent,
    tx: &mpsc::UnboundedSender<AppEvent>,
  ) -> bool {
    match key.code {
      KeyCode::Esc => return true,
      KeyCode::Tab | KeyCode::Down => {
        self.credentials.selected = (self.credentials.selected + 1) % CredentialField::ALL.len();
      }
      KeyCode::BackTab | KeyCode::Up => {
        self.credentials.selected = self
          .credentials
          .selected
          .checked_sub(1)
          .unwrap_or(CredentialField::ALL.len() - 1);
      }
      KeyCode::Enter => self.begin_connect(tx),
      KeyCode::Backspace => {
        let field = CredentialField::ALL[self.credentials.selected];
        self.credentials.value_mut(field).pop();
      }
      KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
        let field = CredentialField::ALL[self.credentials.selected];
        self.credentials.value_mut(field).clear();
      }
      KeyCode::Char(character)
        if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
      {
        let field = CredentialField::ALL[self.credentials.selected];
        self.credentials.value_mut(field).push(character);
      }
      _ => {}
    }
    false
  }

  fn handle_browser_key(&mut self, key: KeyEvent, tx: &mpsc::UnboundedSender<AppEvent>) -> bool {
    match key.code {
      KeyCode::Char('q') | KeyCode::Esc => return true,
      KeyCode::Up | KeyCode::Char('k') => self.change_selection(-1),
      KeyCode::Down | KeyCode::Char('j') => self.change_selection(1),
      KeyCode::PageUp => self.change_selection(-10),
      KeyCode::PageDown => self.change_selection(10),
      KeyCode::Home => self.selected = 0,
      KeyCode::End if !self.entries.is_empty() => self.selected = self.entries.len() - 1,
      KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.activate_selected(tx),
      KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => self.go_to_parent(tx),
      KeyCode::Char(' ') => self.toggle_pause(),
      KeyCode::Char('+') | KeyCode::Char('=') => self.change_volume(5),
      KeyCode::Char('-') => self.change_volume(-5),
      KeyCode::Char('n') => self.play_adjacent(true, tx),
      KeyCode::Char('p') => self.play_adjacent(false, tx),
      KeyCode::Char('x') => self.stop(),
      KeyCode::Char('o') | KeyCode::Char('O') => self.cycle_repeat_mode(),
      KeyCode::Char('v') | KeyCode::Char('V') => self.toggle_visualizer(),
      KeyCode::Char('r') => self.begin_list(self.prefix.clone(), tx),
      KeyCode::Char('c') => self.open_credentials(),
      _ => {}
    }
    false
  }

  fn handle_key(&mut self, key: KeyEvent, tx: &mpsc::UnboundedSender<AppEvent>) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
      return true;
    }
    match self.screen {
      Screen::Credentials => self.handle_credentials_key(key, tx),
      Screen::Browser => self.handle_browser_key(key, tx),
    }
  }

  fn handle_event(&mut self, event: AppEvent, tx: &mpsc::UnboundedSender<AppEvent>) -> bool {
    match event {
      AppEvent::Key(key) => return self.handle_key(key, tx),
      AppEvent::Connected { generation, result } if generation == self.generation => {
        self.busy = false;
        match result {
          Ok((storage, entries)) => {
            let count = entries.len();
            self.storage = Some(storage);
            self.entries = entries;
            self.selected = 0;
            self.prefix.clear();
            self.screen = Screen::Browser;
            self.credentials.clear_sensitive();
            self.status = listing_status(count);
          }
          Err(message) => self.status = message,
        }
      }
      AppEvent::Listed {
        generation,
        prefix,
        result,
      } if generation == self.generation => {
        self.busy = false;
        match result {
          Ok(entries) => {
            let count = entries.len();
            self.entries = entries;
            self.prefix = prefix;
            self.selected = 0;
            self.status = listing_status(count);
          }
          Err(message) => self.status = message,
        }
      }
      AppEvent::TrackReady {
        generation,
        index,
        key,
        name,
        result,
      } if generation == self.generation => {
        self.track_task = None;
        self.pending_track = None;
        self.busy = false;
        match result {
          Ok(prepared) => {
            let duration = prepared.duration;
            let cache = Arc::clone(&prepared.cache);
            let player = match self.player.as_mut() {
              Some(player) => player,
              None => match AudioPlayer::new(self.volume) {
                Ok(player) => self.player.insert(player),
                Err(error) => {
                  self.status = format!("Could not open audio output: {error:#}");
                  return false;
                }
              },
            };
            player.play(prepared);
            self.selected = index.min(self.entries.len().saturating_sub(1));
            self.now_playing = Some(CurrentTrack {
              key,
              name: name.clone(),
              duration,
              cache,
            });
            self.paused = false;
            self.completed = false;
            self.finished_handled = false;
            self.status = format!("Playing {name}");
          }
          Err(message) => {
            self.completed = self.player.as_ref().is_some_and(AudioPlayer::is_finished);
            self.status = message;
          }
        }
      }
      _ => {}
    }
    false
  }

  fn tick(&mut self, tx: &mpsc::UnboundedSender<AppEvent>) {
    if self.now_playing.is_some() && !self.paused && !self.completed {
      self.animation_frame = (self.animation_frame + 1) % 5;
    }
    if self.show_visualizer {
      self.spectrum = self
        .player
        .as_mut()
        .map_or([0.0; crate::visualizer::BAR_COUNT], AudioPlayer::spectrum);
    }

    let finished = self.player.as_ref().is_some_and(AudioPlayer::is_finished);
    if self.now_playing.is_some()
      && !self.paused
      && !self.busy
      && !self.finished_handled
      && finished
    {
      self.finished_handled = true;
      self.completed = true;
      match completion_action(&mut self.repeat_mode) {
        CompletionAction::Replay => self.begin_replay(tx),
        CompletionAction::Advance => {
          if let Some(index) = self.adjacent_track(true) {
            self.selected = index;
            self.begin_play(index, tx);
          } else {
            self.status = "Finished the last track in this folder.".to_owned();
          }
        }
      }
    }
  }
}

enum AppEvent {
  Key(KeyEvent),
  Connected {
    generation: u64,
    result: Result<(Storage, Vec<Entry>), String>,
  },
  Listed {
    generation: u64,
    prefix: String,
    result: Result<Vec<Entry>, String>,
  },
  TrackReady {
    generation: u64,
    index: usize,
    key: String,
    name: String,
    result: Result<PreparedTrack, String>,
  },
}

struct TerminalSession {
  terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
  fn enter() -> Result<Self> {
    enable_raw_mode().context("enabling terminal raw mode")?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
      let _ = disable_raw_mode();
      return Err(error).context("entering alternate screen");
    }
    let terminal = Terminal::new(CrosstermBackend::new(stdout)).context("creating terminal")?;
    Ok(Self { terminal })
  }
}

impl Drop for TerminalSession {
  fn drop(&mut self) {
    let _ = disable_raw_mode();
    let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
    let _ = self.terminal.show_cursor();
  }
}

pub async fn run() -> Result<()> {
  let mut terminal = TerminalSession::enter()?;
  let mut app = App::default();
  let running = Arc::new(AtomicBool::new(true));
  let (tx, mut rx) = mpsc::unbounded_channel();
  spawn_input_thread(Arc::clone(&running), tx.clone());
  let mut tick = tokio::time::interval(Duration::from_millis(150));

  let result = async {
    loop {
      terminal.terminal.draw(|frame| ui::draw(frame, &app))?;
      tokio::select! {
        _ = tick.tick() => app.tick(&tx),
        Some(event) = rx.recv() => {
          if app.handle_event(event, &tx) {
            break;
          }
        }
      }
    }
    Ok::<_, anyhow::Error>(())
  }
  .await;

  running.store(false, Ordering::Relaxed);
  app.stop();
  result
}

fn spawn_input_thread(running: Arc<AtomicBool>, tx: mpsc::UnboundedSender<AppEvent>) {
  std::thread::spawn(move || {
    while running.load(Ordering::Relaxed) {
      if event::poll(Duration::from_millis(100)).unwrap_or(false) {
        if let Ok(TerminalEvent::Key(key)) = event::read() {
          if key.kind != KeyEventKind::Release {
            let _ = tx.send(AppEvent::Key(key));
          }
        }
      }
    }
  });
}

fn parent_prefix(prefix: &str) -> String {
  let without_slash = prefix.trim_end_matches('/');
  without_slash
    .rfind('/')
    .map_or_else(String::new, |index| without_slash[..=index].to_owned())
}

fn listing_status(count: usize) -> String {
  match count {
    0 => "This folder has no supported audio files or subfolders.".to_owned(),
    1 => "Loaded 1 item.".to_owned(),
    _ => format!("Loaded {count} items."),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parent_navigation_preserves_directory_separator() {
    assert_eq!(parent_prefix("Artists/Album/"), "Artists/");
    assert_eq!(parent_prefix("Artists/"), "");
    assert_eq!(parent_prefix(""), "");
  }

  #[test]
  fn loop_once_replays_then_returns_to_normal_advancement() {
    let mut mode = RepeatMode::Once;
    assert_eq!(completion_action(&mut mode), CompletionAction::Replay);
    assert_eq!(mode, RepeatMode::Off);
    assert_eq!(completion_action(&mut mode), CompletionAction::Advance);
  }

  #[test]
  fn loop_forever_keeps_replaying() {
    let mut mode = RepeatMode::Forever;
    assert_eq!(completion_action(&mut mode), CompletionAction::Replay);
    assert_eq!(mode, RepeatMode::Forever);
    assert_eq!(completion_action(&mut mode), CompletionAction::Replay);
  }

  #[test]
  fn rapid_navigation_advances_from_the_pending_target() {
    let app = App {
      entries: vec![
        Entry {
          kind: EntryKind::Track,
          key: "one.mp3".to_owned(),
          name: "one.mp3".to_owned(),
          size: 1,
        },
        Entry {
          kind: EntryKind::Folder,
          key: "folder/".to_owned(),
          name: "folder".to_owned(),
          size: 0,
        },
        Entry {
          kind: EntryKind::Track,
          key: "two.mp3".to_owned(),
          name: "two.mp3".to_owned(),
          size: 1,
        },
        Entry {
          kind: EntryKind::Track,
          key: "three.mp3".to_owned(),
          name: "three.mp3".to_owned(),
          size: 1,
        },
      ],
      pending_track: Some(2),
      ..App::default()
    };

    assert_eq!(app.adjacent_track(true), Some(3));
    assert_eq!(app.adjacent_track(false), Some(0));
  }
}
