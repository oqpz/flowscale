use flowscale_config::{normalize_scale_factor, persistence::write_atomic, GameProfile};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{fs, path::Path};

pub const MAX_BYTES: usize = 256 * 1024;
const MAX_ENTRIES: usize = 100;
const LOCAL: &[&str] = &[
    "id",
    "title",
    "path",
    "preferred_gpu_id",
    "output_display_id",
    "auto_scale",
    "auto_scale_delay",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ConfigAction {
    List,
    Import { document: String },
    Export { profile_id: String },
    Load { id: u64, profile_id: String },
    Delete { id: u64 },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableConfig {
    version: u32,
    name: String,
    options: Map<String, Value>,
}

impl PortableConfig {
    pub fn from_profile(profile: &GameProfile) -> Result<Self, String> {
        let mut profile = profile.clone();
        profile.scale_factor = normalize_scale_factor(profile.scaling_type, profile.scale_factor);
        let mut options = profile_fields(&profile)?;
        for key in LOCAL {
            options.remove(*key);
        }
        let name = profile.title.trim().chars().take(120).collect::<String>();
        Ok(Self {
            version: 1,
            name: if name.is_empty() {
                "FlowScale profile".into()
            } else {
                name
            },
            options,
        })
    }

    pub fn parse(document: &str) -> Result<Self, String> {
        if document.len() > MAX_BYTES {
            return Err("A config must be 256 KiB or smaller.".into());
        }
        let config: Self = serde_json::from_str(document.trim_start_matches('\u{feff}'))
            .map_err(|error| format!("Invalid config JSON: {error}"))?;
        config.apply(&GameProfile::default())?;
        Ok(config)
    }

    pub fn apply(&self, target: &GameProfile) -> Result<GameProfile, String> {
        if self.version != 1 {
            return Err(format!("Unsupported config version {}.", self.version));
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 120 {
            return Err("A config name must contain 1–120 characters.".into());
        }
        if self.options.is_empty() {
            return Err("The config contains no options.".into());
        }
        let mut fields = profile_fields(target)?;
        for (key, value) in &self.options {
            if LOCAL.contains(&key.as_str()) || !fields.contains_key(key) {
                return Err(format!("Unsupported portable option {key:?}."));
            }
            fields.insert(key.clone(), value.clone());
        }
        let profile: GameProfile = serde_json::from_value(Value::Object(fields))
            .map_err(|error| format!("Invalid config value: {error}"))?;
        if !(1.0..=20.0).contains(&profile.lsfg3_multiplier)
            || !(10.0..=1000.0).contains(&profile.lsfg3_target)
            || !(25..=100).contains(&profile.lsfg_flow_scale)
            || !(0..=10).contains(&profile.sharpness)
            || !(0..=4).contains(&profile.ls1_sharpness)
            || !(1..=15).contains(&profile.max_frame_latency)
            || !(0..=2).contains(&profile.queue_target)
            || [
                profile.crop_left,
                profile.crop_top,
                profile.crop_right,
                profile.crop_bottom,
            ]
            .iter()
            .any(|n| *n < 0)
            || (profile.scale_factor
                - normalize_scale_factor(profile.scaling_type, profile.scale_factor))
            .abs()
                > 0.0001
        {
            return Err(
                "One or more config options are outside FlowScale's supported bounds.".into(),
            );
        }
        Ok(profile)
    }
}

fn profile_fields(profile: &GameProfile) -> Result<Map<String, Value>, String> {
    match serde_json::to_value(profile).map_err(|error| error.to_string())? {
        Value::Object(fields) => Ok(fields),
        _ => Err("Could not encode the profile.".into()),
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    id: u64,
    config: PortableConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSummary {
    id: u64,
    name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ConfigLibrary {
    entries: Vec<Entry>,
    next_id: u64,
}

impl ConfigLibrary {
    pub fn read(path: &Path) -> Result<Self, String> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(format!("Could not read the config library: {error}")),
        };
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("The config library could not be loaded: {error}"))
    }

    pub fn list(&self) -> Vec<ConfigSummary> {
        self.entries
            .iter()
            .map(|entry| ConfigSummary {
                id: entry.id,
                name: entry.config.name.clone(),
            })
            .collect()
    }

    pub fn import(&mut self, config: PortableConfig) -> Result<(), String> {
        if self.entries.len() >= MAX_ENTRIES {
            return Err(
                "Delete a saved config before importing another. The library holds up to 100."
                    .into(),
            );
        }
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or("Config library ID limit reached.")?;
        self.entries.push(Entry {
            id: self.next_id,
            config,
        });
        Ok(())
    }

    pub fn get(&self, id: u64) -> Result<&PortableConfig, String> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| &entry.config)
            .ok_or_else(|| "The saved config no longer exists.".into())
    }

    pub fn delete(&mut self, id: u64) -> Result<(), String> {
        self.get(id)?;
        self.entries.retain(|entry| entry.id != id);
        Ok(())
    }
}

pub fn same_destination(first: &Path, second: &Path) -> bool {
    fn canonical_target(path: &Path) -> Option<std::path::PathBuf> {
        path.canonicalize().ok().or_else(|| {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            Some(parent.canonicalize().ok()?.join(path.file_name()?))
        })
    }

    match (canonical_target(first), canonical_target(second)) {
        (Some(first), Some(second)) => first
            .to_string_lossy()
            .eq_ignore_ascii_case(&second.to_string_lossy()),
        _ => false,
    }
}

pub fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let mut json = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    json.push(b'\n');
    write_atomic(path, &json).map_err(|error| format!("Could not save JSON: {error}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn portable_config_preserves_cpu_processing_choice() {
        let cpu = GameProfile { cpu_vulkan: true, ..GameProfile::default() };
        let document = serde_json::to_string(&PortableConfig::from_profile(&cpu).unwrap()).unwrap();
        let imported = PortableConfig::parse(&document).unwrap().apply(&GameProfile::default()).unwrap();
        assert!(imported.cpu_vulkan);
    }
    use super::*;

    #[test]
    fn portable_options_round_trip_without_local_fields() {
        let source = GameProfile {
            lsfg_flow_scale: 75,
            path: "private.exe".into(),
            ..GameProfile::default()
        };
        let target = GameProfile {
            id: "game".into(),
            title: "Game".into(),
            path: "game.exe".into(),
            preferred_gpu_id: 2,
            output_display_id: 3,
            auto_scale: true,
            auto_scale_delay: 7,
            ..GameProfile::default()
        };
        let json = serde_json::to_string(&PortableConfig::from_profile(&source).unwrap()).unwrap();
        assert!(!json.contains("private.exe"));
        let applied = PortableConfig::parse(&json)
            .unwrap()
            .apply(&target)
            .unwrap();
        let expected = GameProfile {
            lsfg_flow_scale: 75,
            ..target
        };
        assert_eq!(applied, expected);
    }

    #[test]
    fn validates_input_and_keeps_same_name_entries_independent() {
        let json = r#"{"version":1,"name":"Preset","options":{"lsfg_flow_scale":75}}"#;
        assert!(PortableConfig::parse(&json.replace("75", "150")).is_err());
        assert!(PortableConfig::parse(&json.replace("flow_scale", "unknown")).is_err());
        assert!(PortableConfig::parse(&json.replace("\"version\":1", "\"version\":2")).is_err());
        assert!(
            PortableConfig::parse(&json.replace("lsfg_flow_scale", "preferred_gpu_id")).is_err()
        );
        assert!(PortableConfig::parse(&" ".repeat(MAX_BYTES + 1)).is_err());
        let mut library = ConfigLibrary::default();
        library
            .import(PortableConfig::parse(json).unwrap())
            .unwrap();
        library
            .import(PortableConfig::parse(json).unwrap())
            .unwrap();
        let dir =
            std::env::temp_dir().join(format!("flowscale-config-library-{}", std::process::id()));
        let path = dir.join("configs.json");
        let source = dir.join("imported.json");
        write_json(&source, &PortableConfig::parse(json).unwrap()).unwrap();
        assert!(same_destination(&path, &dir.join("CONFIGS.JSON")));
        assert!(!same_destination(&path, &source));
        write_json(&path, &library).unwrap();
        let mut reloaded = ConfigLibrary::read(&path).unwrap();
        assert_eq!(reloaded.list().len(), 2);
        reloaded.delete(1).unwrap();
        write_json(&path, &reloaded).unwrap();
        assert_eq!(ConfigLibrary::read(&path).unwrap().list()[0].id, 2);
        assert!(source.is_file());
        assert!(write_json(&path.join("blocked.json"), &reloaded).is_err());
        fs::remove_file(path).unwrap();
        fs::remove_file(source).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
