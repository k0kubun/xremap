extern crate serde_yaml;

use std::{collections::HashMap, error, fs, path::Path, time::SystemTime};

use evdev::Key;
use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};
use serde::{Deserialize, Deserializer};

use keymap::Keymap;
use modmap::Modmap;

use crate::config::absconfig::AbsConfig;

use self::{
    key::parse_key,
    keymap::{build_keymap_table, KeymapEntry},
};

pub mod absconfig;
pub mod action;
pub mod application;
mod key;
pub mod key_action;
pub mod key_press;
pub mod keymap;
mod modmap;

pub mod remap;
#[cfg(test)]
mod tests;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    // Config interface
    #[serde(default = "Vec::new")]
    pub modmap: Vec<Modmap>,
    #[serde(default = "Vec::new")]
    pub keymap: Vec<Keymap>,
    #[serde(default = "default_mode")]
    pub default_mode: String,
    #[serde(deserialize_with = "deserialize_virtual_modifiers", default = "Vec::new")]
    pub virtual_modifiers: Vec<Key>,
    #[serde(default)]
    pub keypress_delay_ms: u64,
    #[serde(default = "AbsConfig::new")]
    pub absolute: AbsConfig,

    // Internals
    #[serde(skip)]
    pub modify_time: Option<SystemTime>,
    #[serde(skip)]
    pub keymap_table: HashMap<Key, Vec<KeymapEntry>>,
}

pub fn load_config(filename: &Path) -> Result<Config, Box<dyn error::Error>> {
    let yaml = fs::read_to_string(&filename)?;
    let mut config: Config = serde_yaml::from_str(&yaml)?;

    // Timestamp for --watch=config
    config.modify_time = filename.metadata()?.modified().ok();

    // Convert keymap for efficient keymap lookup
    config.keymap_table = build_keymap_table(&config.keymap);

    Ok(config)
}

pub fn config_watcher(watch: bool, file: &Path) -> anyhow::Result<Option<Inotify>> {
    if watch {
        let inotify = Inotify::init(InitFlags::IN_NONBLOCK)?;
        inotify.add_watch(
            file.parent().expect("config file has a parent directory"),
            AddWatchFlags::IN_CREATE | AddWatchFlags::IN_MOVED_TO,
        )?;
        inotify.add_watch(file, AddWatchFlags::IN_MODIFY)?;
        Ok(Some(inotify))
    } else {
        Ok(None)
    }
}

fn default_mode() -> String {
    "default".to_string()
}

fn deserialize_virtual_modifiers<'de, D>(deserializer: D) -> Result<Vec<Key>, D::Error>
where
    D: Deserializer<'de>,
{
    let key_strs = Vec::<String>::deserialize(deserializer)?;
    let mut keys: Vec<Key> = vec![];
    for key_str in key_strs {
        keys.push(parse_key(&key_str).map_err(serde::de::Error::custom)?);
    }
    return Ok(keys);
}
