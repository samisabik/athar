import re
import os
import subprocess
import sys
import tempfile
import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import librosa
import numpy as np
import onnxruntime as ort
import soundfile as sf
from tokenizers import Tokenizer

MODEL_ID = "laion/larger_clap_music_and_speech"
CLAP_SR = 48000
CLAP_SECONDS = 10
ANALYSIS_SECONDS = 30
PEAK_BINS = 120
MAX_CLIPS = 6
# The audio model gains little from more threads on one clip or from batches, so clips are run
# side by side instead: EMBED_CALLERS at once, each on EMBED_THREADS threads. Measured on a
# 10-core Mac, that takes about 31 ms a clip against 55 ms for one batched call on every core.
EMBED_THREADS = 2
EMBED_CALLERS = max(1, (os.cpu_count() or 2) // EMBED_THREADS)

CATEGORIES = {
    "Kick": "a kick drum hit",
    "Snare": "a snare drum hit",
    "Clap": "a hand clap drum sample",
    "Hi-hat": "a hi-hat cymbal",
    "Cymbal": "a crash or ride cymbal",
    "Tom": "a tom drum hit",
    "Percussion": "a percussion hit such as shaker, conga or rim",
    "Drum loop": "a drum loop with a beat",
    "Bass": "a bass synth or bass guitar",
    "808": "an 808 sub bass",
    "Synth lead": "a synth lead melody",
    "Synth pad": "an ambient synth pad",
    "Pluck": "a plucked synth note",
    "Chord": "a chord stab",
    "Keys": "piano or electric piano",
    "Guitar": "an electric or acoustic guitar",
    "Strings": "orchestral strings",
    "Brass": "brass instruments",
    "Vocal": "a human voice singing or speaking",
    "FX": "a sound effect",
    "Riser": "a riser or sweep building up",
    "Impact": "a cinematic impact or boom",
    "Texture": "an atmospheric drone or texture",
    "Foley": "a foley sound of everyday objects",
    "Birds": "birds singing or calling",
    "Insects": "insects buzzing or crickets chirping",
    "Animals": "animal sounds such as dogs, cows or frogs",
    "Water": "running water, a stream or waves",
    "Rain": "rain falling",
    "Wind": "wind blowing",
    "Thunder": "thunder and storm",
    "Fire": "a crackling fire",
    "Footsteps": "footsteps walking",
    "Voices": "people talking in a crowd",
    "Traffic": "cars and city traffic",
    "Machinery": "machines, engines or motors",
    "Bells": "bells ringing",
    "Doors": "a door opening, closing or creaking",
    "Metal": "metal objects hitting or scraping",
    "Wood": "wooden objects knocking or creaking",
    "Paper": "paper rustling or crumpling",
    "Room tone": "quiet room tone or background hum",
}

NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]


def read_any(path):
    """Decode any audio file to (channels x samples, rate). libsndfile reads WAV, AIFF,
    FLAC, OGG and MP3; anything else (m4a, aac, caf...) goes through macOS's afconvert."""
    try:
        y, sr = sf.read(path, dtype="float32", always_2d=True)
    except Exception:
        fd, tmp = tempfile.mkstemp(suffix=".wav")
        os.close(fd)
        try:
            subprocess.run(["afconvert", "-f", "WAVE", "-d", "LEF32", str(path), tmp],
                           check=True, capture_output=True)
            y, sr = sf.read(tmp, dtype="float32", always_2d=True)
        except subprocess.CalledProcessError:
            raise RuntimeError(f"Cannot decode {os.path.basename(str(path))}") from None
        finally:
            os.unlink(tmp)
    return y.T, int(sr)


def decode_mono(path, sr, seconds):
    y, file_sr = read_any(path)
    y = y.mean(axis=0)[: int(seconds * file_sr)]
    if file_sr != sr:
        y = librosa.resample(y, orig_sr=file_sr, target_sr=sr)
    return y.astype(np.float32)


def load_audio(path, sr=CLAP_SR, seconds=ANALYSIS_SECONDS):
    try:
        y, _ = librosa.load(path, sr=sr, mono=True, duration=seconds)
    except Exception:
        y = decode_mono(path, sr, seconds)
    return y.astype(np.float32)


def file_info(path):
    try:
        info = sf.info(path)
        return float(info.duration), int(info.samplerate), int(info.channels)
    except Exception:
        pass
    y, sr = read_any(path)
    return y.shape[1] / sr, sr, y.shape[0]


def note_name(hz):
    if not hz:
        return None
    midi = int(round(librosa.hz_to_midi(hz)))
    return f"{NOTE_NAMES[midi % 12]}{midi // 12 - 1}"


def analyze(y, sr, duration, name=""):
    """Signal descriptors used for filtering and sorting."""
    if y.size == 0:
        return {}
    peak = float(np.max(np.abs(y))) or 1e-9
    rms = float(np.sqrt(np.mean(y ** 2))) or 1e-9

    y22 = librosa.resample(y, orig_sr=sr, target_sr=22050)
    centroid = float(np.mean(librosa.feature.spectral_centroid(y=y22, sr=22050)))
    onsets = librosa.onset.onset_detect(y=y22, sr=22050, units="time")

    pitch = None
    seg = y22[: 22050]
    if seg.size >= 4096:
        flatness = float(np.mean(librosa.feature.spectral_flatness(y=seg)))
        if flatness < 0.05:
            f0 = librosa.yin(seg, fmin=30, fmax=2000, sr=22050)
            f0 = f0[np.isfinite(f0)]
            if f0.size:
                p = float(np.median(f0))
                spread = float(np.std(librosa.hz_to_midi(f0)))
                if 32 < p < 1900 and spread < 1.0:
                    pitch = p

    tempo, is_loop = None, False
    m = re.search(r"(?<!\d)(\d{2,3})\s*bpm", name, re.I)
    name_bpm = float(m.group(1)) if m and 50 <= int(m.group(1)) <= 220 else None
    if duration >= 1.5 and len(onsets) >= 4:
        env = librosa.onset.onset_strength(y=y22, sr=22050)
        t, beats = librosa.beat.beat_track(onset_envelope=env, sr=22050)
        t = float(np.atleast_1d(t)[0])
        pulse = float(env[beats].mean() / (env.mean() + 1e-9)) if len(beats) >= 4 else 0
        # A loop is cut to a whole number of beats, so its length implies a
        # tempo. Accept it only when that agrees with the detected beat.
        if pulse > 2 and t > 0:
            for k in (1, 2, 0.5):
                cands = [n * 60 / duration for n in (4, 8, 12, 16, 24, 32, 48, 64)]
                cands = [c for c in cands if 60 <= c <= 200 and abs(c - t * k) / c < 0.04]
                if cands:
                    tempo, is_loop = round(min(cands, key=lambda c: abs(c - t * k)), 2), True
                    break
    if name_bpm:
        tempo, is_loop = name_bpm, True
    elif re.search(r"loop", name, re.I) and duration >= 1:
        is_loop = True

    return {
        "peak_db": float(20 * np.log10(peak)),
        "rms_db": float(20 * np.log10(rms)),
        "crest": peak / rms,
        "brightness": centroid,
        "pitch_hz": pitch,
        "tempo": tempo,
        "onsets": int(len(onsets)),
        "is_loop": int(is_loop),
    }


def peaks(y, bins=PEAK_BINS):
    if y.size == 0:
        return bytes(bins)
    chunks = np.array_split(np.abs(y), bins)
    vals = np.array([c.max() if c.size else 0 for c in chunks])
    vals = vals / (vals.max() or 1)
    return (vals * 255).astype(np.uint8).tobytes()


def file_peaks(path, y, duration, bins=PEAK_BINS):
    """Waveform overview of the whole file. y holds only the first ANALYSIS_SECONDS, so a
    longer file is streamed from disk."""
    if duration <= ANALYSIS_SECONDS:
        return peaks(y, bins)
    try:
        vals, pos = np.zeros(bins), 0
        with sf.SoundFile(path) as f:
            total = max(1, f.frames)
            for block in f.blocks(blocksize=1 << 16, dtype="float32", always_2d=True):
                m = np.abs(block).max(axis=1)
                idx = np.minimum((pos + np.arange(len(m))) * bins // total, bins - 1)
                used, starts = np.unique(idx, return_index=True)
                vals[used] = np.maximum(vals[used], np.maximum.reduceat(m, starts))
                pos += len(m)
        return (vals / (vals.max() or 1) * 255).astype(np.uint8).tobytes()
    except Exception:
        return peaks(y, bins)


def clips(path, y, duration):
    """What CLAP hears of a file, as 48 kHz mono clips: the sound itself when it is short,
    otherwise up to MAX_CLIPS ten-second clips spread evenly from start to end."""
    if duration <= CLAP_SECONDS * 1.5:
        return [y]
    n = min(MAX_CLIPS, int(np.ceil(duration / CLAP_SECONDS)))
    starts = np.linspace(0, duration - CLAP_SECONDS, n)
    try:
        out = []
        with sf.SoundFile(path) as f:
            for s in starts:
                f.seek(int(s * f.samplerate))
                c = f.read(int(CLAP_SECONDS * f.samplerate), dtype="float32", always_2d=True).mean(axis=1)
                out.append(c if f.samplerate == CLAP_SR else librosa.resample(c, orig_sr=f.samplerate, target_sr=CLAP_SR))
    except Exception:
        full = decode_mono(path, CLAP_SR, duration + 1)
        out = [full[int(s * CLAP_SR): int((s + CLAP_SECONDS) * CLAP_SR)] for s in starts]
    out = [c for c in out if c.size]
    return out or [y]


def model_dir():
    """The built app ships the model in its bundle; running from source uses the copy
    written by packaging/export_model.py."""
    for d in (Path(getattr(sys, "_MEIPASS", "")) / "model", Path(__file__).resolve().parents[1] / "build" / "model"):
        if (d / "audio.onnx").exists():
            return d
    raise RuntimeError("CLAP model not found. Run: uv run python packaging/export_model.py")


# CLAP audio input, as computed by the model's own feature extractor ("rand_trunc" with
# "repeatpad"): 10 s at 48 kHz, repeated to fill, then a 64-band Slaney log-mel spectrogram.
N_FFT, HOP, N_MELS = 1024, 480, 64
MEL = librosa.filters.mel(sr=CLAP_SR, n_fft=N_FFT, n_mels=N_MELS, fmin=50, fmax=14000, htk=False, norm="slaney")
HANN = 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(N_FFT) / N_FFT)


def clap_features(y):
    n = CLAP_SR * CLAP_SECONDS
    y = np.asarray(y, np.float64)[:n]
    if 0 < len(y) < n:
        y = np.tile(y, n // len(y))
    y = np.pad(y, (0, n - len(y)))
    x = np.pad(y, N_FFT // 2, mode="reflect")
    frames = np.lib.stride_tricks.sliding_window_view(x, N_FFT)[::HOP]
    power = np.abs(np.fft.rfft(frames * HANN, axis=1)) ** 2
    return (10 * np.log10(np.maximum(power @ MEL.T, 1e-10)))[None].astype(np.float32)


class Engine:
    device = "cpu"

    def __init__(self):
        d = model_dir()
        opts = ort.SessionOptions()
        opts.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
        audio_opts = ort.SessionOptions()
        audio_opts.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
        audio_opts.intra_op_num_threads = EMBED_THREADS
        self.audio = ort.InferenceSession(str(d / "audio.onnx"), audio_opts, providers=["CPUExecutionProvider"])
        self._callers = ThreadPoolExecutor(EMBED_CALLERS, thread_name_prefix="athar-embed")
        self.text = ort.InferenceSession(str(d / "text.onnx"), opts, providers=["CPUExecutionProvider"])
        self.tokenizer = Tokenizer.from_file(str(d / "tokenizer.json"))
        self.tokenizer.enable_padding(pad_id=self.tokenizer.token_to_id("<pad>"), pad_token="<pad>")
        self.tokenizer.enable_truncation(512)
        self.lock = threading.Lock()
        self.category_names = list(CATEGORIES)
        self.category_matrix = self.embed_texts(list(CATEGORIES.values()))

    @staticmethod
    def _normalize(v):
        v = np.asarray(v, np.float32)
        return v / np.maximum(np.linalg.norm(v, axis=-1, keepdims=True), 1e-12)

    def embed_texts(self, texts):
        enc = self.tokenizer.encode_batch(list(texts))
        feed = {"input_ids": np.array([e.ids for e in enc], np.int64),
                "attention_mask": np.array([e.attention_mask for e in enc], np.int64)}
        with self.lock:
            return self._normalize(self.text.run(None, feed)[0])

    def embed_audio(self, waveforms):
        def one(w):
            return self.audio.run(None, {"input_features": clap_features(w)[None]})[0]
        with self.lock:
            out = list(self._callers.map(one, waveforms))
        return self._normalize(np.concatenate(out))
