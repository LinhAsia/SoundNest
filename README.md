# SoundNest 🎵

> A high-performance personal audio workstation, soundboard, and sound effect manager written in pure Rust.

SoundNest is an all-in-one desktop sound utility designed for content creators, streamers, and audio enthusiasts. It combines instant soundboard playback, advanced visual audio trimming, real-time microphone pitch monitoring, virtual microphone streaming, AI vocal separation, and media downloading into a single lightweight, responsive desktop application.

---

## ✨ Features

- **🚀 Instant Soundboard & Hotkeys**:
  - Global hotkeys for instant sound triggering in games and apps.
  - Playlist management with reordering, shuffle, loop, and master playlist volume.
  - Folder categorization, tag filtering, and rapid search.

- **✂️ Waveform & Audio Editor**:
  - High-precision visual waveform display with millisecond-accurate scrubbing.
  - Non-destructive trimming, cutting, and timeline segment manipulation.
  - Real-time audio effects: pitch shifting, speed alteration, normalization, volume multiplier, reverb, and 8D stereo rotation.

- **🎙️ Real-time Pitch Monitor**:
  - Low-latency WASAPI microphone capture and pitch detection.
  - Musical note recognition (e.g., C4, D#5) with sharp mode toggles.
  - Compact, always-on-top desktop overlay widget.

- **🔊 Virtual Microphone Routing (Stream Input)**:
  - Route system audio and microphone into a virtual cable (VB-CABLE).
  - Broadcast sound effects directly into Discord, OBS, Twitch, or game voice chat.

- **🎼 AI Vocal & Music Separation**:
  - Local stem extraction powered by demucs-rs.
  - Separate songs into isolated vocals and instrumental backing tracks.

- **🤖 Gemini AI Text-to-Speech**:
  - Generate customized speech from text prompts using Google Gemini TTS.
  - Multiple voice profiles and prompt presets.

- **📥 Media Downloader**:
  - Fast audio extraction from YouTube, SoundCloud, Myinstants, and other platforms via yt-dlp.
  - Integrated Myinstants search and 1-click sound import.

- **⚡ 1-Click Auto-Updater**:
  - Background version checking via GitHub releases.
  - Seamless background download and self-relaunch into the updated version without command prompt flashing.

---

## 🛠️ Built With

- **Language**: [Rust](https://www.rust-lang.org/) (2024 edition)
- **GUI Framework**: [eframe](https://github.com/emilk/egui/tree/master/crates/eframe) / [egui](https://github.com/emilk/egui) (Glow backend)
- **Audio Engine**: [rodio](https://github.com/RustAudio/rodio), [wasapi](https://github.com/michaelfranzl/wasapi-rs), [hound](https://github.com/ruuda/hound)
- **Networking**: [ureq](https://github.com/algesten/ureq) with native-tls

---

## 🚀 Getting Started

### Prerequisites
- Windows 10 or Windows 11 (64-bit)
- [Rust toolchain](https://rustup.rs/) (stable, 1.85+)

### Build from Source

```powershell
# Clone the repository
git clone https://github.com/LinhAsia/soundnest.git
cd soundnest

# Build optimized release binary
cargo build --release
```

The compiled binary will be located at `target/release/SoundNest.exe`.

---

## 📌 Notice

This is a personal open-source project created and maintained by **LinhAsia**. Distributed under the MIT License.
