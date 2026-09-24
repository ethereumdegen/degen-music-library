use crate::storage::DownloadedTrack;
use anyhow::{Context, Result};
use rodio::cpal::traits::HostTrait;
use rodio::{Decoder, DeviceSinkBuilder, Player, Source};
use std::io::BufReader;
use std::sync::{mpsc, Arc};
use std::time::Duration;

const DEVICE_OPEN_TIMEOUT: Duration = Duration::from_secs(5);

pub struct PreparedTrack {
  source: Box<dyn Source + Send>,
  pub duration: Option<Duration>,
}

pub struct AudioPlayer {
  player: Arc<Player>,
  _keepalive: mpsc::Sender<()>,
  volume: u8,
}

impl AudioPlayer {
  pub fn new(volume: u8) -> Result<Self> {
    let (init_tx, init_rx) = mpsc::channel::<Result<Player, String>>();
    let (keepalive_tx, keepalive_rx) = mpsc::channel::<()>();

    std::thread::Builder::new()
      .name("degen-music-audio".to_owned())
      .spawn(move || {
        let opened = (|| {
          let device = rodio::cpal::default_host()
            .default_output_device()
            .context("no audio output device available")?;
          let mut sink = DeviceSinkBuilder::from_device(device)?.open_sink_or_fallback()?;
          sink.log_on_drop(false);
          Ok::<_, anyhow::Error>((Player::connect_new(sink.mixer()), sink))
        })();

        match opened {
          Ok((player, sink)) => {
            if init_tx.send(Ok(player)).is_ok() {
              let _ = keepalive_rx.recv();
            }
            drop(sink);
          }
          Err(error) => {
            let _ = init_tx.send(Err(format!("{error:#}")));
          }
        }
      })
      .context("spawning audio output thread")?;

    let player = init_rx
      .recv_timeout(DEVICE_OPEN_TIMEOUT)
      .context("audio output did not open in time")?
      .map_err(anyhow::Error::msg)?;
    player.pause();
    player.set_volume(volume_gain(volume));
    Ok(Self {
      player: Arc::new(player),
      _keepalive: keepalive_tx,
      volume,
    })
  }

  pub fn play(&self, prepared: PreparedTrack) {
    self.player.clear();
    self.player.append(prepared.source);
    self.player.play();
  }

  pub fn toggle_pause(&self) -> bool {
    if self.player.is_paused() {
      self.player.play();
      false
    } else {
      self.player.pause();
      true
    }
  }

  pub fn stop(&self) {
    self.player.clear();
  }

  pub fn set_volume(&mut self, volume: u8) {
    self.volume = volume.min(100);
    self.player.set_volume(volume_gain(self.volume));
  }

  pub fn position(&self) -> Duration {
    self.player.get_pos()
  }

  pub fn is_finished(&self) -> bool {
    self.player.empty()
  }
}

pub fn prepare(downloaded: DownloadedTrack) -> Result<PreparedTrack> {
  if downloaded.byte_len == 0 {
    anyhow::bail!("audio object is empty");
  }
  let reader = BufReader::new(downloaded.file);
  let mut builder = Decoder::builder()
    .with_data(reader)
    .with_byte_len(downloaded.byte_len)
    .with_seekable(true);
  if let Some(extension) = downloaded.extension.as_deref() {
    builder = builder.with_hint(extension);
  }
  if let Some(content_type) = downloaded.content_type.as_deref() {
    builder = builder.with_mime_type(content_type);
  }
  let decoder = builder
    .build()
    .map_err(|error| anyhow::anyhow!("decoding audio object: {error}"))?;
  let duration = decoder.total_duration();
  Ok(PreparedTrack {
    source: Box::new(decoder),
    duration,
  })
}

fn volume_gain(percent: u8) -> f32 {
  let linear = f32::from(percent.min(100)) / 100.0;
  linear * linear
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn volume_curve_keeps_bounds_and_is_perceptual() {
    assert_eq!(volume_gain(0), 0.0);
    assert_eq!(volume_gain(100), 1.0);
    assert_eq!(volume_gain(200), 1.0);
    assert!((volume_gain(50) - 0.25).abs() < f32::EPSILON);
  }
}
