use crate::config::action::{Action, Actions};
use crate::config::application::OnlyOrNot;
use crate::config::key_press::{KeyPress, Modifier, ModifierState};
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keymap {
    #[serde(default = "String::new")]
    pub name: String,
    #[serde(deserialize_with = "deserialize_remap")]
    pub remap: HashMap<KeyPress, Vec<Action>>,
    pub application: Option<OnlyOrNot>,
    pub window: Option<OnlyOrNot>,
}

fn deserialize_remap<'de, D>(deserializer: D) -> Result<HashMap<KeyPress, Vec<Action>>, D::Error>
where
    D: Deserializer<'de>,
{
    let remap = HashMap::<KeyPress, Actions>::deserialize(deserializer)?;
    Ok(remap
        .into_iter()
        .flat_map(|(key_press, actions)| {
            expand_modifiers(key_press)
                .into_iter()
                .map(|k| (k, actions.clone().into_vec()))
                .collect::<Vec<(KeyPress, Vec<Action>)>>()
        })
        .collect())
}

// Expand ModifierState::Either to Left and Right. Not leaving Either to save some space and computation.
// Note that we currently don't have `Both`. Does anybody need it?
pub fn expand_modifiers(key_press: KeyPress) -> Vec<KeyPress> {
    if key_press.shift == ModifierState::Either {
        expand_modifier(key_press, &Modifier::Shift)
    } else if key_press.control == ModifierState::Either {
        expand_modifier(key_press, &Modifier::Control)
    } else if key_press.alt == ModifierState::Either {
        expand_modifier(key_press, &Modifier::Alt)
    } else if key_press.windows == ModifierState::Either {
        expand_modifier(key_press, &Modifier::Windows)
    } else {
        vec![key_press]
    }
}

fn expand_modifier(key_press: KeyPress, modifier: &Modifier) -> Vec<KeyPress> {
    vec![
        change_modifier(key_press.clone(), modifier, ModifierState::Left),
        change_modifier(key_press, modifier, ModifierState::Right),
    ]
    .into_iter()
    .flat_map(expand_modifiers)
    .collect()
}

fn change_modifier(key_press: KeyPress, modifier: &Modifier, state: ModifierState) -> KeyPress {
    let mut shift = key_press.shift.clone();
    let mut control = key_press.control.clone();
    let mut alt = key_press.alt.clone();
    let mut windows = key_press.windows.clone();

    match modifier {
        Modifier::Shift => shift = state,
        Modifier::Control => control = state,
        Modifier::Alt => alt = state,
        Modifier::Windows => windows = state,
    }

    KeyPress {
        key: key_press.key,
        shift,
        control,
        alt,
        windows,
    }
}
