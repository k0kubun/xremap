extern crate evdev;
extern crate nix;

use anyhow::bail;
use derive_where::derive_where;
use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{AttributeSet, BusType, Device, FetchEventsSynced, InputId, Key, RelativeAxisType};
use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};

use std::collections::HashMap;
use std::error::Error;
use std::fs::read_dir;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::prelude::AsRawFd;
use std::path::PathBuf;
use std::{io, process};

use anyhow::bail;
use derive_where::derive_where;
use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{
    AbsoluteAxisType, AttributeSet, AttributeSetRef, Device, FetchEventsSynced, Key, MiscType, PropType,
    RelativeAxisType, UinputAbsSetup,
};
use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};
use serde::Deserialize;

use crate::config::absconfig::AbsConfig;

static MOUSE_BTNS: [&str; 20] = [
    "BTN_MISC",
    "BTN_0",
    "BTN_1",
    "BTN_2",
    "BTN_3",
    "BTN_4",
    "BTN_5",
    "BTN_6",
    "BTN_7",
    "BTN_8",
    "BTN_9",
    "BTN_MOUSE",
    "BTN_LEFT",
    "BTN_RIGHT",
    "BTN_MIDDLE",
    "BTN_SIDE",
    "BTN_EXTRA",
    "BTN_FORWARD",
    "BTN_BACK",
    "BTN_TASK",
];

static TABLET_BTNS: [Key; 17] = [
    Key::BTN_TOOL_PEN,
    Key::BTN_TOOL_AIRBRUSH,
    Key::BTN_TOOL_BRUSH,
    Key::BTN_TOOL_PENCIL,
    Key::BTN_TOUCH,
    Key::BTN_STYLUS,
    Key::BTN_STYLUS2,
    Key::BTN_0,
    Key::BTN_1,
    Key::BTN_2,
    Key::BTN_3,
    Key::BTN_4,
    Key::BTN_5,
    Key::BTN_6,
    Key::BTN_7,
    Key::BTN_8,
    Key::BTN_9,
];

// Credit: https://github.com/mooz/xkeysnail/blob/bf3c93b4fe6efd42893db4e6588e5ef1c4909cfb/xkeysnail/output.py#L10-L32
pub fn output_device(bus_type: Option<BusType>) -> Result<VirtualDevice, Box<dyn Error>> {
    let mut keys: AttributeSet<Key> = AttributeSet::new();
    for code in Key::KEY_RESERVED.code()..Key::BTN_TRIGGER_HAPPY40.code() {
        let key = Key::new(code);
        let name = format!("{:?}", key);
        let heap_name = name.as_str();
        if name.starts_with("KEY_") || MOUSE_BTNS.contains(&heap_name) || TABLET_BTNS.contains(&key) {
            keys.insert(key);
        }
    }

    let mut relative_axes: AttributeSet<RelativeAxisType> = AttributeSet::new();
    relative_axes.insert(RelativeAxisType::REL_X);
    relative_axes.insert(RelativeAxisType::REL_Y);
    relative_axes.insert(RelativeAxisType::REL_HWHEEL);
    relative_axes.insert(RelativeAxisType::REL_WHEEL);
    relative_axes.insert(RelativeAxisType::REL_MISC);

    let device = VirtualDeviceBuilder::new()?
        // These are taken from https://docs.rs/evdev/0.12.0/src/evdev/uinput.rs.html#183-188
        .input_id(InputId::new(bus_type.unwrap_or(BusType::BUS_USB), 0x1234, 0x5678, 0x111))
        .name(&InputDevice::current_name())
        .with_keys(&keys)?
        .with_relative_axes(&relative_axes)?
        .build()?;
    Ok(device)
}

pub fn tablet_device(abs_config: &AbsConfig) -> Result<VirtualDevice, Box<dyn Error>> {
    let mut keys: AttributeSet<Key> = AttributeSet::new();
    for code in Key::KEY_RESERVED.code()..Key::BTN_TRIGGER_HAPPY40.code() {
        let key = Key::new(code);
        let name = format!("{:?}", key);
        let heap_name = name.as_str();
        if TABLET_BTNS.contains(&key) {
            keys.insert(key);
        }
    }

    let mut props: AttributeSet<PropType> = AttributeSet::new();
    props.insert(PropType::POINTER);

    let mut msc: AttributeSet<MiscType> = AttributeSet::new();
    msc.insert(MiscType::MSC_SCAN);

    let x = UinputAbsSetup::new(AbsoluteAxisType::ABS_X, abs_config.x.into_evdev_abs_info());
    let x_tilt = UinputAbsSetup::new(AbsoluteAxisType::ABS_TILT_X, abs_config.tilt_x.into_evdev_abs_info());
    let y = UinputAbsSetup::new(AbsoluteAxisType::ABS_Y, abs_config.y.into_evdev_abs_info());
    let y_tilt = UinputAbsSetup::new(AbsoluteAxisType::ABS_TILT_Y, abs_config.tilt_y.into_evdev_abs_info());
    let pressure = UinputAbsSetup::new(AbsoluteAxisType::ABS_PRESSURE, abs_config.pressure.into_evdev_abs_info());

    let device = VirtualDeviceBuilder::new()?
        .name(&InputDevice::current_name_tablet())
        .with_keys(&keys)?
        .with_absolute_axis(&x)?
        .with_absolute_axis(&y)?
        .with_absolute_axis(&x_tilt)?
        .with_absolute_axis(&y_tilt)?
        .with_absolute_axis(&pressure)?
        .with_properties(&*props)?
        .with_msc(&*msc)?
        .build()?;
    Ok(device)
}

pub fn device_watcher(watch: bool) -> anyhow::Result<Option<Inotify>> {
    if watch {
        let inotify = Inotify::init(InitFlags::IN_NONBLOCK)?;
        inotify.add_watch("/dev/input", AddWatchFlags::IN_CREATE | AddWatchFlags::IN_ATTRIB)?;
        Ok(Some(inotify))
    } else {
        Ok(None)
    }
}

pub fn get_input_devices(
    device_opts: &[String],
    ignore_opts: &[String],
    mouse: bool,
    watch: bool,
) -> anyhow::Result<HashMap<PathBuf, InputDevice>> {
    let mut devices: Vec<_> = InputDevice::devices()?.collect();
    devices.sort();

    println!("Selecting devices from the following list:");
    println!("{}", SEPARATOR);
    devices.iter().for_each(InputDevice::print);
    println!("{}", SEPARATOR);

    if device_opts.is_empty() {
        if mouse {
            print!("Selected keyboards and mice automatically since --device options weren't specified");
        } else {
            print!("Selected keyboards automatically since --device options weren't specified");
        }
    } else {
        print!("Selected devices matching {:?}", device_opts);
    };
    if ignore_opts.is_empty() {
        println!(":")
    } else {
        println!(", ignoring {:?}:", ignore_opts);
    }

    let devices: Vec<_> = devices
        .into_iter()
        // filter map needed for mutable access
        // alternative is `Vec::retain_mut` whenever that gets stabilized
        .filter_map(|mut device| {
            // filter out any not matching devices and devices that error on grab
            (device.is_input_device(device_opts, ignore_opts, mouse) && device.grab()).then(|| device)
        })
        .collect();

    println!("{}", SEPARATOR);
    if devices.is_empty() {
        if watch {
            println!("warning: No device was selected, but --watch is waiting for new devices.");
        } else {
            bail!("No device was selected!");
        }
    } else {
        devices.iter().for_each(InputDevice::print);
    }
    println!("{}", SEPARATOR);

    Ok(devices.into_iter().map(From::from).collect())
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub enum DeviceType {
    Tablet,
    Other,
}

#[derive_where(PartialEq, PartialOrd, Ord)]
pub struct InputDevice {
    path: PathBuf,
    #[derive_where(skip)]
    device: Device,
}

impl Eq for InputDevice {}

impl TryFrom<PathBuf> for InputDevice {
    type Error = io::Error;

    fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
        let fname = path
            .file_name()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        if fname.as_bytes().starts_with(b"event") {
            Ok(Self {
                device: Device::open(&path)?,
                path,
            })
        } else {
            Err(io::ErrorKind::InvalidInput.into())
        }
    }
}

impl From<InputDevice> for (PathBuf, InputDevice) {
    fn from(device: InputDevice) -> Self {
        (device.path.clone(), device)
    }
}

impl AsRawFd for InputDevice {
    fn as_raw_fd(&self) -> std::os::unix::prelude::RawFd {
        self.device.as_raw_fd()
    }
}

/// Device Wrappers Abstractions
impl InputDevice {
    pub fn grab(&mut self) -> bool {
        if let Err(error) = self.device.grab() {
            println!("Failed to grab device '{}' at '{}' due to: {error}", self.device_name(), self.path.display());
            false
        } else {
            true
        }
    }

    pub fn ungrab(&mut self) {
        if let Err(error) = self.device.ungrab() {
            println!("Failed to ungrab device '{}' at '{}' due to: {error}", self.device_name(), self.path.display());
        }
    }

    pub fn fetch_events(&mut self) -> io::Result<FetchEventsSynced> {
        self.device.fetch_events()
    }

    fn device_name(&self) -> &str {
        self.device.name().unwrap_or("<Unnamed device>")
    }

    pub fn bus_type(&self) -> BusType {
        self.device.input_id().bus_type()
    }
}

impl InputDevice {
    pub fn is_input_device(&self, device_filter: &[String], ignore_filter: &[String], mouse: bool) -> bool {
        if self.device_name() == Self::current_name() || self.device_name() == Self::current_name_tablet() {
            return false;
        }
        (if device_filter.is_empty() {
            self.is_keyboard() || (mouse && self.is_mouse())
        } else {
            self.matches(device_filter)
        }) && (ignore_filter.is_empty() || !self.matches(ignore_filter))
    }

    // We can't know the device path from evdev::enumerate(). So we re-implement it.
    fn devices() -> io::Result<impl Iterator<Item = InputDevice>> {
        Ok(read_dir("/dev/input")?.filter_map(|entry| {
            // Allow "Permission denied" when opening the current process's own device.
            InputDevice::try_from(entry.ok()?.path()).ok()
        }))
    }

    fn current_name() -> String {
        format!("xremap pid={}", process::id())
    }

    fn current_name_tablet() -> String {
        format!("xremap tablet pid={}", process::id())
    }

    fn matches(&self, filter: &[String]) -> bool {
        // Force unmatch its own device
        if self.device_name() == Self::current_name() {
            return false;
        }

        for device_opt in filter {
            let device_opt = device_opt.as_str();

            // Check exact matches for explicit selection
            if self.path.as_os_str() == device_opt || self.device_name() == device_opt {
                return true;
            }
            // eventXX shorthand for /dev/input/eventXX
            if device_opt.starts_with("event")
                && self.path.file_name().expect("every device path has a file name") == device_opt
            {
                return true;
            }
            // Allow partial matches for device names
            if self.device_name().contains(device_opt) {
                return true;
            }
        }
        false
    }

    fn is_keyboard(&self) -> bool {
        // Credit: https://github.com/mooz/xkeysnail/blob/bf3c93b4fe6efd42893db4e6588e5ef1c4909cfb/xkeysnail/input.py#L17-L32
        match self.device.supported_keys() {
            Some(keys) => {
                keys.contains(Key::KEY_SPACE)
                && keys.contains(Key::KEY_A)
                && keys.contains(Key::KEY_Z)
                // BTN_MOUSE
                && !keys.contains(Key::BTN_LEFT)
            }
            None => false,
        }
    }

    // https://docs.kernel.org/input/event-codes.html?highlight=event+types#tablets
    pub(crate) fn is_tablet(&self) -> bool {
        let has_tablet_axes = match self.device.supported_absolute_axes() {
            Some(axes) => axes.contains(AbsoluteAxisType::ABS_Y) && axes.contains(AbsoluteAxisType::ABS_X),
            None => false,
        };

        has_tablet_axes
            && match self.device.supported_keys() {
                Some(keys) => keys.contains(Key::BTN_TOOL_PEN) && keys.contains(Key::BTN_TOUCH),
                None => false,
            }
    }

    fn is_mouse(&self) -> bool {
        self.device
            .supported_keys()
            .map_or(false, |keys| keys.contains(Key::BTN_LEFT))
    }

    pub fn print(&self) {
        println!("{:18}: {}", self.path.display(), self.device_name())
    }
}

const SEPARATOR: &str = "------------------------------------------------------------------------------";
