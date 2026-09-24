use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const BAR_COUNT: usize = 64;
const FFT_SIZE: usize = 2048;
const SAMPLE_CAPACITY: usize = FFT_SIZE * 2;
const FLUSH_SIZE: usize = 256;

#[derive(Default)]
struct Samples {
  values: VecDeque<f32>,
  sample_rate: u32,
  generation: u64,
}

#[derive(Clone, Default)]
pub struct SampleTap {
  inner: Arc<Mutex<Samples>>,
}

impl SampleTap {
  pub fn clear(&self) {
    let mut samples = self.inner.lock().unwrap_or_else(|error| error.into_inner());
    samples.values.clear();
    samples.generation = samples.generation.wrapping_add(1);
  }

  fn configure(&self, sample_rate: u32) {
    let mut samples = self.inner.lock().unwrap_or_else(|error| error.into_inner());
    samples.sample_rate = sample_rate;
    samples.values.clear();
    samples.generation = samples.generation.wrapping_add(1);
  }

  fn push(&self, pending: &mut Vec<f32>) {
    let mut samples = self.inner.lock().unwrap_or_else(|error| error.into_inner());
    samples.values.extend(pending.drain(..));
    let overflow = samples.values.len().saturating_sub(SAMPLE_CAPACITY);
    samples.values.drain(..overflow);
    samples.generation = samples.generation.wrapping_add(1);
  }
}

pub struct AnalyzedSource<S> {
  input: S,
  tap: SampleTap,
  channels: usize,
  channel_index: usize,
  frame_sum: f32,
  pending: Vec<f32>,
}

impl<S: Source> AnalyzedSource<S> {
  pub fn new(input: S, tap: SampleTap) -> Self {
    let channels = usize::from(input.channels().get());
    tap.configure(input.sample_rate().get());
    Self {
      input,
      tap,
      channels,
      channel_index: 0,
      frame_sum: 0.0,
      pending: Vec::with_capacity(FLUSH_SIZE),
    }
  }

  fn flush(&mut self) {
    if !self.pending.is_empty() {
      self.tap.push(&mut self.pending);
    }
  }
}

impl<S: Source> Iterator for AnalyzedSource<S> {
  type Item = Sample;

  fn next(&mut self) -> Option<Self::Item> {
    let Some(sample) = self.input.next() else {
      self.flush();
      return None;
    };

    self.frame_sum += sample;
    self.channel_index += 1;
    if self.channel_index == self.channels {
      self.pending.push(self.frame_sum / self.channels as f32);
      self.channel_index = 0;
      self.frame_sum = 0.0;
      if self.pending.len() == FLUSH_SIZE {
        self.flush();
      }
    }
    Some(sample)
  }

  fn size_hint(&self) -> (usize, Option<usize>) {
    self.input.size_hint()
  }
}

impl<S: Source> Source for AnalyzedSource<S> {
  fn current_span_len(&self) -> Option<usize> {
    self.input.current_span_len()
  }

  fn channels(&self) -> ChannelCount {
    self.input.channels()
  }

  fn sample_rate(&self) -> SampleRate {
    self.input.sample_rate()
  }

  fn total_duration(&self) -> Option<Duration> {
    self.input.total_duration()
  }

  fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
    self.input.try_seek(position)?;
    self.channel_index = 0;
    self.frame_sum = 0.0;
    self.pending.clear();
    self.tap.clear();
    Ok(())
  }
}

pub struct SpectrumAnalyzer {
  tap: SampleTap,
  fft: Arc<dyn Fft<f32>>,
  input: Vec<Complex32>,
  scratch: Vec<Complex32>,
  window: Vec<f32>,
  bars: [f32; BAR_COUNT],
  last_generation: u64,
}

impl SpectrumAnalyzer {
  pub fn new(tap: SampleTap) -> Self {
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let scratch = vec![Complex32::ZERO; fft.get_inplace_scratch_len()];
    let window = (0..FFT_SIZE)
      .map(|index| {
        let phase = std::f32::consts::TAU * index as f32 / (FFT_SIZE - 1) as f32;
        0.5 - 0.5 * phase.cos()
      })
      .collect();
    Self {
      tap,
      fft,
      input: vec![Complex32::ZERO; FFT_SIZE],
      scratch,
      window,
      bars: [0.0; BAR_COUNT],
      last_generation: 0,
    }
  }

  pub fn update(&mut self) -> [f32; BAR_COUNT] {
    let samples = self
      .tap
      .inner
      .lock()
      .unwrap_or_else(|error| error.into_inner());
    if samples.generation == self.last_generation || samples.values.is_empty() {
      drop(samples);
      for level in &mut self.bars {
        *level *= 0.72;
      }
      return self.bars;
    }

    self.last_generation = samples.generation;
    let sample_rate = samples.sample_rate.max(1);
    let available = samples.values.len().min(FFT_SIZE);
    let padding = FFT_SIZE - available;
    for value in &mut self.input[..padding] {
      *value = Complex32::ZERO;
    }
    for (index, sample) in samples
      .values
      .iter()
      .skip(samples.values.len() - available)
      .copied()
      .enumerate()
    {
      self.input[padding + index] = Complex32::new(sample * self.window[padding + index], 0.0);
    }
    drop(samples);

    self
      .fft
      .process_with_scratch(&mut self.input, &mut self.scratch);

    let nyquist = sample_rate as f32 / 2.0;
    let minimum_hz = 40.0_f32.min(nyquist);
    let maximum_hz = 16_000.0_f32.min(nyquist).max(minimum_hz + 1.0);
    let frequency_ratio = maximum_hz / minimum_hz.max(1.0);

    for index in 0..BAR_COUNT {
      let start_hz = minimum_hz * frequency_ratio.powf(index as f32 / BAR_COUNT as f32);
      let end_hz = minimum_hz * frequency_ratio.powf((index + 1) as f32 / BAR_COUNT as f32);
      let start_bin =
        ((start_hz * FFT_SIZE as f32 / sample_rate as f32) as usize).clamp(1, FFT_SIZE / 2 - 1);
      let end_bin = ((end_hz * FFT_SIZE as f32 / sample_rate as f32).ceil() as usize)
        .clamp(start_bin + 1, FFT_SIZE / 2);
      let magnitude = self.input[start_bin..end_bin]
        .iter()
        .map(|value| value.norm() / FFT_SIZE as f32)
        .fold(0.0_f32, f32::max);
      let decibels = 20.0 * magnitude.max(1.0e-8).log10();
      let target = ((decibels + 72.0) / 72.0).clamp(0.0, 1.0);
      self.bars[index] = if target >= self.bars[index] {
        target
      } else {
        self.bars[index] * 0.82 + target * 0.18
      };
    }
    self.bars
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn fft_places_a_sine_wave_in_the_expected_frequency_band() {
    let tap = SampleTap::default();
    tap.configure(44_100);
    let mut samples = (0..FFT_SIZE)
      .map(|index| (std::f32::consts::TAU * 440.0 * index as f32 / 44_100.0).sin() * 0.5)
      .collect::<Vec<_>>();
    tap.push(&mut samples);

    let mut analyzer = SpectrumAnalyzer::new(tap);
    let bars = analyzer.update();
    let strongest = bars
      .iter()
      .enumerate()
      .max_by(|left, right| left.1.total_cmp(right.1))
      .map(|(index, _)| index)
      .unwrap();

    assert!((24..=29).contains(&strongest));
    assert!(bars[strongest] > 0.5);
  }
}
