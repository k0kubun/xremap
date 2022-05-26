use crate::client::{build_client, WMClient};
use crate::config::action::Action;
use crate::config::application::OnlyOrNot;
use crate::config::key_action::KeyAction;
use crate::config::key_press::{KeyPress, Modifier, ModifierState};
use crate::config::keymap::expand_modifiers;
use crate::Config;
use evdev::uinput::VirtualDevice;
use evdev::{EventType, InputEvent, Key};
use lazy_static::lazy_static;
use log::{debug, error};
use nix::sys::signal;
use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet};
use nix::sys::time::TimeSpec;
use nix::sys::timerfd::{Expiration, TimerFd, TimerSetTimeFlags};
use std::collections::HashMap;
use std::error::Error;
use std::process::{Command, Stdio};
use std::time::Instant;

pub struct EventHandler {
    device: VirtualDevice,
    // Recognize modifier key combinations
    shift: PressState,
    control: PressState,
    alt: PressState,
    windows: PressState,
    // Make sure the original event is released even if remapping changes while holding the key
    pressed_keys: HashMap<Key, Key>,
    // Check the currently active application
    application_client: WMClient,
    application_cache: Option<String>,
    window_cache: Option<String>,
    // State machine for multi-purpose keys
    multi_purpose_keys: HashMap<Key, MultiPurposeKeyState>,
    // Current nested remaps
    override_remap: Option<HashMap<KeyPress, Vec<Action>>>,
    // Key triggered on a timeout of nested remaps
    override_timeout_key: Option<Key>,
    // Trigger a timeout of nested remaps through select(2)
    override_timer: TimerFd,
    // Whether we've called a sigaction for spawing commands or not
    sigaction_set: bool,
    // { set_mark: true }
    mark_set: bool,
    // { escape_next_key: true }
    escape_next_key: bool,
}

impl EventHandler {
    pub fn new(device: VirtualDevice, timer: TimerFd) -> EventHandler {
        EventHandler {
            device,
            shift: PressState::new(),
            control: PressState::new(),
            alt: PressState::new(),
            windows: PressState::new(),
            pressed_keys: HashMap::new(),
            application_client: build_client(),
            application_cache: None,
            window_cache: None,
            multi_purpose_keys: HashMap::new(),
            override_remap: None,
            override_timeout_key: None,
            override_timer: timer,
            sigaction_set: false,
            mark_set: false,
            escape_next_key: false,
        }
    }

    // Handle EventType::KEY
    pub fn on_event(&mut self, event: InputEvent, config: &Config) -> Result<(), Box<dyn Error>> {
        // expire cache
        self.application_cache = None;
        self.window_cache = None;

        let key = Key::new(event.code());
        debug!("=> {}: {:?}", event.value(), &key);

        // Apply modmap
        let mut key_values = if let Some(key_action) = self.find_modmap(config, &key) {
            self.dispatch_keys(key_action, key, event.value())
        } else {
            vec![(key, event.value())]
        };
        self.maintain_pressed_keys(key, event.value(), &mut key_values);
        if !self.multi_purpose_keys.is_empty() {
            key_values = self.flush_timeout_keys(key_values);
        }

        // Apply keymap
        for (key, value) in key_values.into_iter() {
            if MODIFIER_KEYS.contains(&key.code()) {
                self.update_modifier(key.code(), value);
            } else if is_pressed(value) {
                if self.escape_next_key {
                    self.escape_next_key = false
                } else if let Some(actions) = self.find_keymap(config, &key)? {
                    for action in &actions {
                        self.dispatch_action(action, &key)?;
                    }
                    continue;
                }
            }
            self.send_key(&key, value)?;
        }
        Ok(())
    }

    pub fn send_event(&mut self, event: InputEvent) -> std::io::Result<()> {
        if event.event_type() == EventType::KEY {
            debug!("{}: {:?}", event.value(), Key::new(event.code()))
        }
        self.device.emit(&[event])
    }

    pub fn timeout_override(&mut self) -> Result<(), Box<dyn Error>> {
        if let Some(key) = self.override_timeout_key {
            self.send_key(&key, PRESS)?;
            self.send_key(&key, RELEASE)?;
        }
        self.remove_override()
    }

    fn remove_override(&mut self) -> Result<(), Box<dyn Error>> {
        self.override_timer.unset()?;
        self.override_remap = None;
        self.override_timeout_key = None;
        Ok(())
    }

    fn send_key(&mut self, key: &Key, value: i32) -> std::io::Result<()> {
        let event = InputEvent::new(EventType::KEY, key.code(), value);
        self.send_event(event)
    }

    // Repeat/Release what's originally pressed even if remapping changes while holding it
    fn maintain_pressed_keys(&mut self, key: Key, value: i32, events: &mut Vec<(Key, i32)>) {
        // Not handling multi-purpose keysfor now; too complicated
        if events.len() != 1 || value != events[0].1 {
            return;
        }

        let event = events[0];
        if value == PRESS {
            self.pressed_keys.insert(key, event.0);
        } else {
            if let Some(original_key) = self.pressed_keys.get(&key) {
                events[0].0 = original_key.clone();
            }
            if value == RELEASE {
                self.pressed_keys.remove(&key);
            }
        }
    }

    fn dispatch_keys(&mut self, key_action: KeyAction, key: Key, value: i32) -> Vec<(Key, i32)> {
        match key_action {
            KeyAction::Key(modmap_key) => vec![(modmap_key, value)],
            KeyAction::MultiPurposeKey(multi_purpose_key) => {
                if value == PRESS {
                    self.multi_purpose_keys.insert(
                        key,
                        MultiPurposeKeyState {
                            held: multi_purpose_key.held,
                            alone: multi_purpose_key.alone,
                            alone_timeout_at: Some(Instant::now() + multi_purpose_key.alone_timeout),
                        },
                    );
                    return vec![]; // delay the press
                } else if value == REPEAT {
                    if let Some(state) = self.multi_purpose_keys.get_mut(&key) {
                        return state.repeat();
                    }
                } else if value == RELEASE {
                    if let Some(state) = self.multi_purpose_keys.remove(&key) {
                        return state.release();
                    }
                } else {
                    panic!("unexpected key event value: {}", value);
                }
                // fallthrough on state discrepancy
                vec![(key, value)]
            }
        }
    }

    fn flush_timeout_keys(&mut self, key_values: Vec<(Key, i32)>) -> Vec<(Key, i32)> {
        let mut flush = false;
        for (_, value) in key_values.iter() {
            if *value == PRESS {
                flush = true;
                break;
            }
        }

        if flush {
            let mut flushed: Vec<(Key, i32)> = vec![];
            for (_, state) in self.multi_purpose_keys.iter_mut() {
                flushed.extend(state.force_held());
            }
            flushed.extend(key_values);
            flushed
        } else {
            key_values
        }
    }

    fn find_modmap(&mut self, config: &Config, key: &Key) -> Option<KeyAction> {
        for modmap in &config.modmap {
            if let Some(key_action) = modmap.remap.get(key) {
                if let Some(application_matcher) = &modmap.application {
                    if !self.match_application(application_matcher) {
                        continue;
                    }
                }
                return Some(key_action.clone());
            }
        }
        None
    }

    fn find_keymap(&mut self, config: &Config, key: &Key) -> Result<Option<Vec<Action>>, Box<dyn Error>> {
        let key_press = KeyPress {
            key: *key,
            shift: self.shift.to_modifier_state(),
            control: self.control.to_modifier_state(),
            alt: self.alt.to_modifier_state(),
            windows: self.windows.to_modifier_state(),
        };
        if let Some(override_remap) = &self.override_remap {
            if let Some(actions) = override_remap.clone().get(&key_press) {
                self.remove_override()?;
                return Ok(Some(actions.to_vec()));
            } else {
                self.timeout_override()?;
            }
        }
        for keymap in &config.keymap {
            if let Some(actions) = keymap.remap.get(&key_press) {
                if let Some(application_matcher) = &keymap.application {
                    if !self.match_application(application_matcher) {
                        continue;
                    }
                }
                if let Some(window_matcher) = &keymap.window {
                    if !self.match_window(window_matcher) {
                        continue;
                    }
                }
                return Ok(Some(actions.to_vec()));
            }
        }
        Ok(None)
    }

    fn dispatch_action(&mut self, action: &Action, key: &Key) -> Result<(), Box<dyn Error>> {
        match action {
            Action::KeyPress(key_press) => self.send_key_press(key_press)?,
            Action::Remap(action) => {
                let mut override_remap: HashMap<KeyPress, Vec<Action>> = HashMap::new();
                for (key_press, actions) in action.remap.iter() {
                    for key_press in expand_modifiers(key_press.clone()) {
                        override_remap.insert(key_press, actions.to_vec());
                    }
                }
                self.override_remap = Some(override_remap);
                if let Some(timeout) = action.timeout {
                    let expiration = Expiration::OneShot(TimeSpec::from_duration(timeout));
                    self.override_timer.unset()?;
                    self.override_timer.set(expiration, TimerSetTimeFlags::empty())?;
                    self.override_timeout_key = Some(key.clone());
                }
            }
            Action::Launch(command) => self.run_command(command.clone()),
            Action::SetMark(set) => self.mark_set = *set,
            Action::WithMark(key_press) => self.send_key_press(&self.with_mark(key_press))?,
            Action::EscapeNextKey(escape_next_key) => self.escape_next_key = *escape_next_key,
        }
        Ok(())
    }

    fn send_key_press(&mut self, key_press: &KeyPress) -> Result<(), Box<dyn Error>> {
        let next_shift = self.build_state(Modifier::Shift, key_press.shift.clone());
        let next_control = self.build_state(Modifier::Control, key_press.control.clone());
        let next_alt = self.build_state(Modifier::Alt, key_press.alt.clone());
        let next_windows = self.build_state(Modifier::Windows, key_press.windows.clone());

        let prev_shift = self.send_modifier(Modifier::Shift, &next_shift)?;
        let prev_control = self.send_modifier(Modifier::Control, &next_control)?;
        let prev_alt = self.send_modifier(Modifier::Alt, &next_alt)?;
        let prev_windows = self.send_modifier(Modifier::Windows, &next_windows)?;

        self.send_key(&key_press.key, PRESS)?;
        self.send_key(&key_press.key, RELEASE)?;

        self.send_modifier(Modifier::Windows, &prev_windows)?;
        self.send_modifier(Modifier::Alt, &prev_alt)?;
        self.send_modifier(Modifier::Control, &prev_control)?;
        self.send_modifier(Modifier::Shift, &prev_shift)?;
        Ok(())
    }

    fn send_modifier(&mut self, modifier: Modifier, desired: &PressState) -> Result<PressState, Box<dyn Error>> {
        let mut current = match modifier {
            Modifier::Shift => &self.shift,
            Modifier::Control => &self.control,
            Modifier::Alt => &self.alt,
            Modifier::Windows => &self.windows,
        }
        .clone();
        let original = current.clone();
        let left_key = match modifier {
            Modifier::Shift => &SHIFT_KEYS[0],
            Modifier::Control => &CONTROL_KEYS[0],
            Modifier::Alt => &ALT_KEYS[0],
            Modifier::Windows => &WINDOWS_KEYS[0],
        };
        let right_key = match modifier {
            Modifier::Shift => &SHIFT_KEYS[1],
            Modifier::Control => &CONTROL_KEYS[1],
            Modifier::Alt => &ALT_KEYS[1],
            Modifier::Windows => &WINDOWS_KEYS[1],
        };

        if !current.left && desired.left {
            self.send_key(left_key, PRESS)?;
            current.left = true;
        } else if current.left && !desired.left {
            self.send_key(left_key, RELEASE)?;
            current.left = false;
        }

        if !current.right && desired.right {
            self.send_key(right_key, PRESS)?;
            current.right = true;
        } else if current.right && !desired.right {
            self.send_key(right_key, RELEASE)?;
            current.right = false;
        }

        match modifier {
            Modifier::Shift => self.shift = current,
            Modifier::Control => self.control = current,
            Modifier::Alt => self.alt = current,
            Modifier::Windows => self.windows = current,
        };
        Ok(original)
    }

    fn build_state(&self, modifier: Modifier, modifier_state: ModifierState) -> PressState {
        let press_state = match modifier {
            Modifier::Shift => &self.shift,
            Modifier::Control => &self.control,
            Modifier::Alt => &self.alt,
            Modifier::Windows => &self.windows,
        };

        match modifier_state {
            ModifierState::Either => {
                // Choose a PressState closest to the current PressState
                if press_state.left || press_state.right {
                    press_state.clone() // no change is necessary
                } else {
                    // Just press left
                    PressState {
                        left: true,
                        right: false,
                    }
                }
            }
            ModifierState::Left => PressState {
                left: true,
                right: false,
            },
            ModifierState::Right => PressState {
                left: false,
                right: true,
            },
            ModifierState::None => PressState {
                left: false,
                right: false,
            },
        }
    }

    fn with_mark(&self, key_press: &KeyPress) -> KeyPress {
        let mut shift = key_press.shift.clone();
        if self.mark_set && shift == ModifierState::None {
            // We don't leave Either in expand_modifiers(), so just using Left here.
            shift = ModifierState::Left;
        }
        KeyPress {
            key: key_press.key,
            shift,
            control: key_press.control.clone(),
            alt: key_press.alt.clone(),
            windows: key_press.windows.clone(),
        }
    }

    fn run_command(&mut self, command: Vec<String>) {
        if !self.sigaction_set {
            // Avoid defunct processes
            let sig_action = SigAction::new(SigHandler::SigDfl, SaFlags::SA_NOCLDWAIT, SigSet::empty());
            unsafe {
                sigaction(signal::SIGCHLD, &sig_action).expect("Failed to register SIGCHLD handler");
            }
            self.sigaction_set = true;
        }

        debug!("Running command: {:?}", command);
        match Command::new(&command[0])
            .args(&command[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => debug!("Process spawned: {:?}, pid {}", command, child.id()),
            Err(e) => error!("Error running command: {:?}", e),
        }
    }

    fn match_application(&mut self, application_matcher: &OnlyOrNot) -> bool {
        // Lazily fill the wm_class cache
        if self.application_cache.is_none() {
            match self.application_client.current_application() {
                Some(application) => self.application_cache = Some(application),
                None => self.application_cache = Some(String::new()),
            }
        }

        if let Some(application) = &self.application_cache {
            if let Some(application_only) = &application_matcher.only {
                return application_only.contains(application);
            }
            if let Some(application_not) = &application_matcher.not {
                return !application_not.contains(application);
            }
        }
        false
    }

    fn match_window(&mut self, window_matcher: &OnlyOrNot) -> bool {
        // Lazily fill the wm_class cache
        if self.window_cache.is_none() {
            match self.application_client.current_window() {
                Some(window) => self.window_cache = Some(window),
                None => self.window_cache = Some(String::new()),
            }
        }

        if let Some(window) = &self.window_cache {
            if let Some(window_only) = &window_matcher.only {
                return window_only.contains(window);
            }
            if let Some(window_not) = &window_matcher.not {
                return !window_not.contains(window);
            }
        }
        false
    }

    fn update_modifier(&mut self, code: u16, value: i32) {
        if code == Key::KEY_LEFTSHIFT.code() {
            self.shift.left = is_pressed(value)
        } else if code == Key::KEY_RIGHTSHIFT.code() {
            self.shift.right = is_pressed(value)
        } else if code == Key::KEY_LEFTCTRL.code() {
            self.control.left = is_pressed(value)
        } else if code == Key::KEY_RIGHTCTRL.code() {
            self.control.right = is_pressed(value)
        } else if code == Key::KEY_LEFTALT.code() {
            self.alt.left = is_pressed(value)
        } else if code == Key::KEY_RIGHTALT.code() {
            self.alt.right = is_pressed(value)
        } else if code == Key::KEY_LEFTMETA.code() {
            self.windows.left = is_pressed(value)
        } else if code == Key::KEY_RIGHTMETA.code() {
            self.windows.right = is_pressed(value)
        } else {
            panic!("unexpected key {:?} at update_modifier", Key::new(code));
        }
    }
}

lazy_static! {
    static ref MODIFIER_KEYS: [u16; 8] = [
        // Shift
        Key::KEY_LEFTSHIFT.code(),
        Key::KEY_RIGHTSHIFT.code(),
        // Control
        Key::KEY_LEFTCTRL.code(),
        Key::KEY_RIGHTCTRL.code(),
        // Alt
        Key::KEY_LEFTALT.code(),
        Key::KEY_RIGHTALT.code(),
        // Windows
        Key::KEY_LEFTMETA.code(),
        Key::KEY_RIGHTMETA.code(),
    ];

    static ref SHIFT_KEYS: [Key; 2] = [
        Key::new(Key::KEY_LEFTSHIFT.code()),
        Key::new(Key::KEY_RIGHTSHIFT.code()),
    ];
    static ref CONTROL_KEYS: [Key; 2] = [
        Key::new(Key::KEY_LEFTCTRL.code()),
        Key::new(Key::KEY_RIGHTCTRL.code()),
    ];
    static ref ALT_KEYS: [Key; 2] = [
        Key::new(Key::KEY_LEFTALT.code()),
        Key::new(Key::KEY_RIGHTALT.code()),
    ];
    static ref WINDOWS_KEYS: [Key; 2] = [
        Key::new(Key::KEY_LEFTMETA.code()),
        Key::new(Key::KEY_RIGHTMETA.code()),
    ];
}

//---

#[derive(Clone)]
struct PressState {
    left: bool,
    right: bool,
}

impl PressState {
    fn new() -> PressState {
        PressState {
            left: false,
            right: false,
        }
    }

    fn to_modifier_state(&self) -> ModifierState {
        if self.left {
            ModifierState::Left
        } else if self.right {
            ModifierState::Right
        } else {
            ModifierState::None
        }
    }
}

fn is_pressed(value: i32) -> bool {
    value == PRESS || value == REPEAT
}

// InputEvent#value
static RELEASE: i32 = 0;
static PRESS: i32 = 1;
static REPEAT: i32 = 2;

//---

#[derive(Debug)]
struct MultiPurposeKeyState {
    held: Key,
    alone: Key,
    // Some if the first press is still delayed, None if already considered held.
    alone_timeout_at: Option<Instant>,
}

impl MultiPurposeKeyState {
    fn repeat(&mut self) -> Vec<(Key, i32)> {
        if let Some(alone_timeout_at) = &self.alone_timeout_at {
            if Instant::now() < *alone_timeout_at {
                vec![] // still delay the press
            } else {
                self.alone_timeout_at = None; // timeout
                vec![(self.held, PRESS)]
            }
        } else {
            vec![(self.held, REPEAT)]
        }
    }

    fn release(&self) -> Vec<(Key, i32)> {
        if let Some(alone_timeout_at) = &self.alone_timeout_at {
            if Instant::now() < *alone_timeout_at {
                // dispatch the delayed press and this release
                vec![(self.alone, PRESS), (self.alone, RELEASE)]
            } else {
                // too late. dispatch the held key
                vec![(self.held, PRESS), (self.held, RELEASE)]
            }
        } else {
            vec![(self.held, RELEASE)]
        }
    }

    fn force_held(&mut self) -> Vec<(Key, i32)> {
        if self.alone_timeout_at.is_some() {
            self.alone_timeout_at = None;
            vec![(self.held, PRESS)]
        } else {
            vec![]
        }
    }
}
