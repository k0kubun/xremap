use crate::config::application::OnlyOrNot;
use crate::config::key::deserialize_key;
use crate::config::key_action::KeyAction;
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
    pub application: Option<OnlyOrNot>,
    pub window: Option<OnlyOrNot>,
}

fn deserialize_remap<'de, D>(deserializer: D) -> Result<HashMap<Key, KeyAction>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize, Eq, Hash, PartialEq)]
    struct KeyWrapper(#[serde(deserialize_with = "deserialize_key")] Key);

    let v = HashMap::<KeyWrapper, KeyAction>::deserialize(deserializer)?;
    Ok(v.into_iter().map(|(KeyWrapper(k), v)| (k, v)).collect())
}
