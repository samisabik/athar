//! CLAP embeddings through ONNX Runtime: sounds and text land in the same space, so a search is
//! the dot product of a text vector with every sound's vector.

use crate::features::{clap_features, FRAMES, N_MELS};
use ort::environment::Environment;
use ort::ep;
use ort::session::{builder::AutoDevicePolicy, Session};
use ort::value::Tensor;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// ort's builder errors carry the builder and are not Send, so they travel as their message.
fn msg<E: std::fmt::Display>(e: E) -> Error {
    e.to_string().into()
}

fn environment() -> Result<&'static Environment, Error> {
    static ENV: OnceLock<Environment> = OnceLock::new();
    if let Some(e) = ENV.get() {
        return Ok(e);
    }
    let env = ort::init().with_name("athar").build().map_err(msg)?;
    Ok(ENV.get_or_init(|| env))
}

/// Audio clips embedded at once: one model session each, two threads per session. One session
/// gains little from more threads, so several run side by side; they share their packed weights.
/// Measured on a 10-core Mac: 3 sessions index as fast as 5, in a third of the memory.
pub fn default_pool() -> usize {
    (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2) / 2).clamp(1, 3)
}

pub struct Clap {
    audio: Vec<Mutex<Session>>,
    next: std::sync::atomic::AtomicUsize,
    text: Mutex<Session>,
    tokenizer: Tokenizer,
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    v.iter_mut().for_each(|x| *x /= n);
    v
}

impl Clap {
    /// Loads audio.onnx, text.onnx and tokenizer.json from `dir`, with default_pool() audio sessions.
    pub fn load(dir: &Path) -> Result<Self, Error> {
        Self::load_with(dir, default_pool())
    }

    pub fn load_with(dir: &Path, pool: usize) -> Result<Self, Error> {
        let env = environment()?;
        // CPU only, as in the Python engine. On macOS ort otherwise adds the CoreML provider on
        // its own, and that fails on the text model.
        let cpu = || -> Result<_, Error> {
            Session::builder(env)?.with_auto_device(AutoDevicePolicy::PreferCPU).map_err(msg)?.with_execution_providers([ep::CPU::default().build()]).map_err(msg)
        };
        let packed = ort::session::builder::PrepackedWeights::new();
        let audio = (0..pool.max(1))
            .map(|_| -> Result<Session, Error> {
                cpu()?
                    .with_intra_threads(2)
                    .map_err(msg)?
                    .with_prepacked_weights(&packed)
                    .map_err(msg)?
                    .commit_from_file(dir.join("audio.onnx"))
                    .map_err(msg)
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(Mutex::new)
            .collect();
        let text = cpu()?.commit_from_file(dir.join("text.onnx")).map_err(msg)?;
        let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))?;
        let pad_id = tokenizer.token_to_id("<pad>").ok_or("tokenizer has no <pad> token")?;
        tokenizer.with_padding(Some(PaddingParams { pad_id, pad_token: "<pad>".into(), ..Default::default() }));
        tokenizer.with_truncation(Some(TruncationParams { max_length: 512, ..Default::default() }))?;
        Ok(Self { audio, next: Default::default(), text: Mutex::new(text), tokenizer })
    }

    /// One unit-length vector per clip of 48 kHz mono audio.
    pub fn embed_clip(&self, clip: &[f32]) -> Result<Vec<f32>, Error> {
        let input = Tensor::from_array(([1usize, 1, FRAMES, N_MELS], clap_features(clip)))?;
        // A free session if there is one, otherwise wait for the next in turn.
        let mut session = self
            .audio
            .iter()
            .find_map(|s| s.try_lock().ok())
            .unwrap_or_else(|| self.audio[self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % self.audio.len()].lock().unwrap());
        let out = session.run(ort::inputs!["input_features" => input])?;
        let (_, data) = out[0].try_extract_tensor::<f32>()?;
        Ok(normalize(data.to_vec()))
    }

    pub fn pool(&self) -> usize {
        self.audio.len()
    }

    /// A sound made of several clips is the normalized mean of their vectors.
    pub fn embed_sound(&self, clips: &[Vec<f32>]) -> Result<Vec<f32>, Error> {
        let mut sum: Vec<f32> = Vec::new();
        for c in clips {
            let e = self.embed_clip(c)?;
            if sum.is_empty() {
                sum = e;
            } else {
                sum.iter_mut().zip(&e).for_each(|(s, v)| *s += v);
            }
        }
        Ok(normalize(sum))
    }

    pub fn embed_texts(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, Error> {
        let enc = self.tokenizer.encode_batch(texts.to_vec(), true)?;
        let len = enc.first().map(|e| e.get_ids().len()).unwrap_or(0);
        let ids: Vec<i64> = enc.iter().flat_map(|e| e.get_ids().iter().map(|&v| v as i64)).collect();
        let mask: Vec<i64> = enc.iter().flat_map(|e| e.get_attention_mask().iter().map(|&v| v as i64)).collect();
        let shape = [texts.len(), len];
        let mut session = self.text.lock().unwrap();
        let out = session.run(ort::inputs![
            "input_ids" => Tensor::from_array((shape, ids))?,
            "attention_mask" => Tensor::from_array((shape, mask))?,
        ])?;
        let (shape, data) = out[0].try_extract_tensor::<f32>()?;
        let dim = shape[1] as usize;
        Ok(data.chunks(dim).map(|c| normalize(c.to_vec())).collect())
    }
}
