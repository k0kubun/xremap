use crate::config::application::Application;
use crate::config::key::deserialize_key;
use crate::config::key_action::KeyAction;
use crate::config::key_press::KeyPress;
use crate::DeviceType;
use evdev::Key;
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modmap {
    #[serde(default = "String::new")]
    pub name: String,
    #[serde(deserialize_with = "deserialize_remap")]
    pub remap: HashMap<Key, KeyAction>,
    pub application: Option<Application>,
    #[serde(deserialize_with = "deserialize_targets", default = "HashMap::new")]
    pub targets: HashMap<Key, DeviceType>,
}

#[derive(Deserialize, Eq, Hash, PartialEq)]
struct KeyWrapper(#[serde(deserialize_with = "deserialize_key")] Key);

fn deserialize_remap<'de, D>(deserializer: D) -> Result<HashMap<Key, KeyAction>, D::Error>
where
    D: Deserializer<'de>,
{
    let v = HashMap::<KeyWrapper, KeyAction>::deserialize(deserializer)?;
    Ok(v.into_iter().map(|(KeyWrapper(k), v)| (k, v)).collect())
}

fn deserialize_targets<'de, D>(deserializer: D) -> Result<HashMap<Key, DeviceType>, D::Error>
where
    D: Deserializer<'de>,
{
    let v = HashMap::<KeyWrapper, DeviceType>::deserialize(deserializer)?;
    Ok(v.into_iter().map(|(KeyWrapper(k), v)| (k, v)).collect())
}
