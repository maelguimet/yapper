//! App config load/save (`~/.config/yapper/config.toml`).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub stt: SttConfig,
    pub tts: TtsConfig,
    pub read_aloud: ReadAloudConfig,
    pub hotkeys: HotkeysConfig,
    pub models: ModelsConfig,
    pub paths: PathsConfig,
    /// Mic / capture settings. Absent in older configs → default.
    #[serde(default)]
    pub audio: AudioConfig,
}

/// Input capture preferences.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AudioConfig {
    /// Pulse/PipeWire source name for ffmpeg `-i`. Empty = system default.
    #[serde(default)]
    pub mic_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SttConfig {
    /// `small` | `medium`
    pub model: String,
    /// `auto` | `en` | `fr`
    pub language: String,
    pub copy_transcript: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TtsConfig {
    pub model: String,
    /// `auto` | `en` | `fr`
    pub language: String,
    pub tone: String,
    pub voice: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadAloudConfig {
    /// `selection` | `clipboard`
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HotkeysConfig {
    pub read_aloud: String,
    pub push_to_talk: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelsConfig {
    pub dir: String,
    pub voices_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PathsConfig {
    /// Optional PYTHONPATH root for workers.
    /// Empty = import from `python_bin` site-packages (user install).
    /// Dev checkout: set to repo `python/` or use `cargo run` resolution.
    pub python_root: String,
    /// Interpreter that has yapper workers installed (install venv or dev `.venv`).
    pub python_bin: String,
}

impl Default for Config {
    fn default() -> Self {
        let data = default_data_dir();
        Self {
            stt: SttConfig {
                model: "small".into(),
                language: "auto".into(),
                copy_transcript: true,
            },
            tts: TtsConfig {
                model: "chatterbox-multilingual".into(),
                language: "auto".into(),
                tone: "neutral".into(),
                voice: "eve".into(),
            },
            read_aloud: ReadAloudConfig {
                source: "selection".into(),
            },
            hotkeys: HotkeysConfig {
                read_aloud: "Super+Shift+S".into(),
                push_to_talk: "Super+Shift+R".into(),
            },
            models: ModelsConfig {
                dir: data.join("models").to_string_lossy().into(),
                voices_dir: data.join("voices").to_string_lossy().into(),
            },
            paths: PathsConfig {
                // User install: empty root + XDG venv bin (workers in site-packages).
                // Dev: `resolve_python_*` falls back to repo `python/` and `.venv`.
                python_root: String::new(),
                python_bin: data.join("venv/bin/python").to_string_lossy().into(),
            },
            audio: AudioConfig {
                mic_source: String::new(),
            },
        }
    }
}

impl Config {
    pub fn config_path() -> PathBuf {
        let cfg = dirs::config_dir()
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".config")
            })
            .join("yapper");
        cfg.join("config.toml")
    }

    pub fn load_or_default() -> Result<Self> {
        let path = Self::config_path();
        if path.is_file() {
            Self::load(&path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        let cfg: Config = toml::from_str(&raw).context("parse config.toml")?;
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let raw = toml::to_string_pretty(self).context("serialize config")?;
        fs::write(path, raw).with_context(|| format!("write config {}", path.display()))?;
        Ok(())
    }

    pub fn save_default_location(&self) -> Result<()> {
        self.save(&Self::config_path())
    }
}

pub fn default_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".local/share")
        })
        .join("yapper")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn round_trip_toml() {
        let cfg = Config::default();
        let raw = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&raw).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn save_and_load_file() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("yapper-cfg-test-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let mut cfg = Config::default();
        cfg.stt.model = "medium".into();
        cfg.tts.tone = "calm".into();
        cfg.hotkeys.read_aloud = "Ctrl+Alt+S".into();
        cfg.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.stt.model, "medium");
        assert_eq!(loaded.tts.tone, "calm");
        assert_eq!(loaded.hotkeys.read_aloud, "Ctrl+Alt+S");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mic_source_round_trip() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("yapper-cfg-mic-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let mut cfg = Config::default();
        cfg.audio.mic_source =
            "alsa_input.usb-FuZhou_Kingwayinfo_CO._LTD_TONOR_TC30_Audio_Device_20200707-00.mono-fallback"
                .into();
        cfg.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.audio.mic_source, cfg.audio.mic_source);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_audio_section_defaults() {
        // Older configs without [audio] must still load.
        let raw = r#"
[stt]
model = "small"
language = "auto"
copy_transcript = true

[tts]
model = "chatterbox-multilingual"
language = "auto"
tone = "neutral"
voice = "eve"

[read_aloud]
source = "selection"

[hotkeys]
read_aloud = "Super+Shift+S"
push_to_talk = "Super+Shift+R"

[models]
dir = "/tmp/models"
voices_dir = "/tmp/voices"

[paths]
python_root = "/tmp/python"
python_bin = "python3"
"#;
        let cfg: Config = toml::from_str(raw).unwrap();
        assert_eq!(cfg.audio.mic_source, "");
        assert_eq!(cfg.stt.model, "small");
    }
}
