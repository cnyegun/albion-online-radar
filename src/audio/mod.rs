//! Plays the bundled alert sounds on the machine running the radar.

mod wav;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;

use crate::log::log_warn;

/// Decodes every WAV once. Files that cannot be used are skipped with a
/// warning so the other sounds keep working.
pub fn load_clips<'a>(files: impl IntoIterator<Item = (String, Vec<u8>)> + 'a) -> HashMap<String, Arc<Vec<f32>>> {
    let mut clips = HashMap::new();
    for (name, raw) in files {
        if !name.to_ascii_lowercase().ends_with(".wav") {
            continue;
        }
        match wav::decode_wav(&raw) {
            Ok(pcm) => {
                let samples =
                    pcm.as_chunks::<2>().0.iter().map(|s| f32::from(i16::from_le_bytes(*s)) / 32768.0).collect();
                clips.insert(name, Arc::new(samples));
            }
            Err(e) => log_warn!("SND", "sound skipped, {name}: {e}"),
        }
    }
    clips
}

#[cfg(feature = "audio")]
mod backend {
    use std::collections::HashMap;
    use std::num::NonZero;
    use std::sync::Arc;
    use std::sync::mpsc::{self, Sender};

    use anyhow::{Result, anyhow};

    use super::wav::{CHANNELS, SAMPLE_RATE};

    struct PlayCmd {
        samples: Arc<Vec<f32>>,
        volume: f32,
    }

    /// Owns the output device on a dedicated thread (audio streams are not
    /// `Send` on every platform) and plays one clip at a time.
    pub struct Player {
        clips: HashMap<String, Arc<Vec<f32>>>,
        tx: Sender<PlayCmd>,
    }

    impl Player {
        pub fn new(clips: HashMap<String, Arc<Vec<f32>>>) -> Result<Self> {
            let (tx, rx) = mpsc::channel::<PlayCmd>();
            let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
            std::thread::Builder::new().name("audio".into()).spawn(move || {
                let sink = match rodio::DeviceSinkBuilder::open_default_sink() {
                    Ok(mut s) => {
                        s.log_on_drop(false);
                        s
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(anyhow!("{e}")));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                let mut current: Option<rodio::Player> = None;
                for cmd in rx {
                    if let Some(p) = current.take() {
                        p.stop();
                    }
                    let channels = NonZero::new(CHANNELS).expect("non-zero");
                    let rate = NonZero::new(SAMPLE_RATE).expect("non-zero");
                    let player = rodio::Player::connect_new(sink.mixer());
                    player.set_volume(cmd.volume);
                    player.append(rodio::buffer::SamplesBuffer::new(channels, rate, cmd.samples.as_slice().to_vec()));
                    player.play();
                    current = Some(player);
                }
            })?;
            ready_rx.recv().map_err(|_| anyhow!("audio thread exited"))??;
            Ok(Self { clips, tx })
        }

        /// Stops whatever is sounding and starts `file`. False when it was not bundled.
        pub fn play(&self, file: &str, volume: f64) -> bool {
            let Some(samples) = self.clips.get(file) else { return false };
            let _ = self.tx.send(PlayCmd { samples: Arc::clone(samples), volume: volume as f32 });
            true
        }
    }
}

#[cfg(feature = "audio")]
pub use backend::Player;

#[cfg(not(feature = "audio"))]
pub struct Player;

#[cfg(not(feature = "audio"))]
impl Player {
    pub fn new(_clips: HashMap<String, Arc<Vec<f32>>>) -> Result<Self> {
        anyhow::bail!("built without the `audio` feature")
    }

    pub fn play(&self, _file: &str, _volume: f64) -> bool {
        false
    }
}

/// Opens the audio device, or returns `None` (with a warning) on a machine
/// without one so the radar keeps running silently.
pub fn new_player(files: impl IntoIterator<Item = (String, Vec<u8>)>) -> Option<Player> {
    let result: Result<Player> = Player::new(load_clips(files));
    match result {
        Ok(p) => Some(p),
        Err(e) => {
            log_warn!("SND", "alert sound unavailable on this machine: {e}");
            None
        }
    }
}
