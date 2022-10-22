use crate::config::action::{Action, Actions};
use crate::config::application::deserialize_string_or_vec;
use crate::config::application::Application;
use crate::config::key_press::KeyPress;
use crate::DeviceType;
use evdev::Key;
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;

use super::key_press::Modifier;

// Config interface
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keymap {
    #[serde(default = "String::new")]
    pub name: String,
    #[serde(deserialize_with = "deserialize_remap")]
    pub remap: HashMap<KeyPress, Vec<Action>>,
    pub application: Option<Application>,
    #[serde(default, deserialize_with = "deserialize_string_or_vec")]
    pub mode: Option<Vec<String>>,
    #[serde(default = "HashMap::new")]
    pub targets: HashMap<KeyPress, DeviceType>,
}

fn deserialize_remap<'de, D>(deserializer: D) -> Result<HashMap<KeyPress, Vec<Action>>, D::Error>
where
    D: Deserializer<'de>,
{
    let remap = HashMap::<KeyPress, Actions>::deserialize(deserializer)?;
    Ok(remap
        .into_iter()
        .map(|(key_press, actions)| (key_press, actions.into_vec()))
        .collect())
}

// Internals for efficient keymap lookup
#[derive(Clone, Debug)]
pub struct KeymapEntry {
    pub actions: Vec<Action>,
    pub modifiers: Vec<Modifier>,
    pub application: Option<Application>,
    pub mode: Option<Vec<String>>,
    pub targets: Option<DeviceType>,
}

// Convert an array of keymaps to a single hashmap whose key is a triggering key.
//
// For each key, Vec<KeymapEntry> is scanned once, matching the exact modifiers,
// and then it's scanned again, allowing extra modifiers.
//
// First matching KeymapEntry wins at each iteration.
pub fn build_keymap_table(keymaps: &Vec<Keymap>) -> HashMap<Key, Vec<KeymapEntry>> {
    let mut table: HashMap<Key, Vec<KeymapEntry>> = HashMap::new();
    for keymap in keymaps {
        for (key_press, actions) in keymap.remap.iter() {
            let mut entries: Vec<KeymapEntry> = match table.get(&key_press.key) {
                Some(entries) => entries.to_vec(),
                None => vec![],
            };

            let target = if let Some(target) = keymap.targets.get(key_press) {
                Some(*target)
            } else {
                None
            };
            entries.push(KeymapEntry {
                actions: actions.to_vec(),
                modifiers: key_press.modifiers.clone(),
                application: keymap.application.clone(),
                mode: keymap.mode.clone(),
                targets: target,
            });
            table.insert(key_press.key, entries);
        }
    }
    return table;
}

// Subset of KeymapEntry for override_remap
#[derive(Clone)]
pub struct OverrideEntry {
    pub actions: Vec<Action>,
    pub modifiers: Vec<Modifier>,
}

// This is executed on runtime unlike build_keymap_table, but hopefully not called so often.
pub fn build_override_table(remap: &HashMap<KeyPress, Vec<Action>>) -> HashMap<Key, Vec<OverrideEntry>> {
    let mut table: HashMap<Key, Vec<OverrideEntry>> = HashMap::new();
    for (key_press, actions) in remap.iter() {
        let mut entries: Vec<OverrideEntry> = match table.get(&key_press.key) {
            Some(entries) => entries.to_vec(),
            None => vec![],
        };
        entries.push(OverrideEntry {
            actions: actions.to_vec(),
            modifiers: key_press.modifiers.clone(),
        });
        table.insert(key_press.key, entries);
    }
    return table;
}
