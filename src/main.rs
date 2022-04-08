use crate::config::Config;
use crate::device::{device_watcher, get_input_devices, output_device};
use crate::event_handler::EventHandler;
use anyhow::{anyhow, bail, Context};
use clap::{AppSettings, ArgEnum, IntoApp, Parser};
use clap_complete::Shell;
use config::{config_watcher, load_config};
use daemonize::Daemonize;
use device::InputDevice;
use evdev::EventType;
use nix::libc::ENODEV;
use nix::sys::inotify::{AddWatchFlags, Inotify};
use nix::sys::select::select;
use nix::sys::select::FdSet;
use nix::sys::timerfd::{ClockId, TimerFd, TimerFlags};
use std::io::stdout;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::PathBuf;

mod client;
mod config;
mod device;
mod event_handler;

#[derive(Parser, Debug)]
#[clap(version, global_setting(AppSettings::DeriveDisplayOrder))]
struct Opts {
    /// Include a device name or path
    #[clap(long, use_delimiter = true)]
    device: Vec<String>,
    /// Ignore a device name or path
    #[clap(long, use_delimiter = true)]
    ignore: Vec<String>,
    /// Targets to watch
    ///
    /// - device: add new devices automatically
    /// - config: reload the config automatically
    #[clap(
        long,
        arg_enum,
        min_values = 0,
        use_delimiter = true,
        require_equals = true,
        default_missing_value = "device",
        verbatim_doc_comment,
        hide_possible_values = true,
        // Separating the help like this is necessary due to 
        // https://github.com/clap-rs/clap/issues/3312
        help = "Targets to watch [possible values: device, config]"
    )]
    watch: Vec<WatchTargets>,
    /// Start xremap as a daemon
    #[clap(long)]
    daemon: bool,
    /// Generate shell completions
    ///
    /// You can use them by storing in your shells completion file or by running
    /// - in bash: eval "$(xremap --completions bash)"
    /// - in fish: xremap --completions fish | source
    #[clap(long, arg_enum, display_order = 100, value_name = "SHELL", verbatim_doc_comment)]
    completions: Option<Shell>,
    /// Config file
    #[clap(required_unless_present = "completions")]
    config: Option<PathBuf>,
}

#[derive(ArgEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum WatchTargets {
    /// Device to add new devices automatically
    Device,
    /// Config to reload the config automatically
    Config,
}

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let Opts {
        device: device_filter,
        ignore: ignore_filter,
        watch,
        daemon,
        config,
        completions,
    } = Opts::parse();

    if daemon {
        let daemonize = Daemonize::new();
        match daemonize.start() {
            Ok(_) => println!("Success, daemonized"),
            Err(e) => eprintln!("Error, {}", e),
        }
    }

    if let Some(shell) = completions {
        clap_complete::generate(shell, &mut Opts::into_app(), "xremap", &mut stdout());
        return Ok(());
    }

    let config_path = config.expect("config is set, if not completions");

    let mut config = match config::load_config(&config_path) {
        Ok(config) => config,
        Err(e) => bail!("Failed to load config '{}': {}", config_path.display(), e),
    };

    let watch_devices = watch.contains(&WatchTargets::Device);
    let watch_config = watch.contains(&WatchTargets::Config);

    let output_device = match output_device() {
        Ok(output_device) => output_device,
        Err(e) => bail!("Failed to prepare an output device: {}", e),
    };
    let timer = TimerFd::new(ClockId::CLOCK_MONOTONIC, TimerFlags::empty())?;
    let timer_fd = timer.as_raw_fd();
    let mut handler = EventHandler::new(output_device, timer);
    let mut input_devices = match get_input_devices(&device_filter, &ignore_filter, watch_devices) {
        Ok(input_devices) => input_devices,
        Err(e) => bail!("Failed to prepare input devices: {}", e),
    };
    let device_watcher = device_watcher(watch_devices).context("Setting up device watcher")?;
    let config_watcher = config_watcher(watch_config, &config_path).context("Setting up config watcher")?;
    let watchers: Vec<_> = device_watcher.iter().chain(config_watcher.iter()).collect();

    loop {
        match 'event_loop: loop {
            let readable_fds = select_readable(input_devices.values(), &watchers, timer_fd)?;
            if readable_fds.contains(timer_fd) {
                if let Err(error) = handler.timeout_override() {
                    println!("Error on remap timeout: {error}")
                }
            }

            for input_device in input_devices.values_mut() {
                if readable_fds.contains(input_device.as_raw_fd()) {
                    match input_device.fetch_events().map_err(|e| (e.raw_os_error(), e)) {
                        Err((Some(ENODEV), _)) => {
                            println!("Found a removed device. Reselecting devices.");
                            break 'event_loop Event::ReloadDevices;
                        }
                        Err((_, error)) => return Err(error).context("Error fetching input events"),
                        Ok(events) => {
                            for event in events {
                                if event.event_type() == EventType::KEY {
                                    handler
                                        .on_event(event, &config)
                                        .map_err(|e| anyhow!("Failed handling {event:?}:\n  {e:?}"))?;
                                } else {
                                    handler.send_event(event)?;
                                }
                            }
                        }
                    }
                }
            }

            if let Some(inotify) = device_watcher {
                if let Ok(events) = inotify.read_events() {
                    input_devices.extend(events.into_iter().filter_map(|event| {
                        event.name.and_then(|name| {
                            let path = PathBuf::from("/dev/input/").join(name);
                            let mut device = InputDevice::try_from(path).ok()?;
                            if device.is_input_device(&device_filter, &ignore_filter) && device.grab() {
                                device.print();
                                Some(device.into())
                            } else {
                                None
                            }
                        })
                    }))
                }
            }
            if let Some(inotify) = config_watcher {
                if let Ok(events) = inotify.read_events() {
                    for event in &events {
                        match (event.mask, &event.name) {
                            // Dir events
                            (_, Some(name))
                                if name == config_path.file_name().expect("Config path has a file name") =>
                            {
                                break 'event_loop Event::ReloadConfig;
                            }
                            // File events
                            (mask, _) if mask.contains(AddWatchFlags::IN_MODIFY) => {
                                break 'event_loop Event::ReloadConfig;
                            }
                            // Unrelated
                            _ => (),
                        }
                    }
                    input_devices.extend(events.into_iter().filter_map(|event| {
                        event.name.and_then(|name| {
                            let path = PathBuf::from("/dev/input/").join(name);
                            let mut device = InputDevice::try_from(path).ok()?;
                            if device.is_input_device(&device_filter, &ignore_filter) && device.grab() {
                                device.print();
                                Some(device.into())
                            } else {
                                None
                            }
                        })
                    }))
                }
            }
        } {
            Event::ReloadDevices => {
                for input_device in input_devices.values_mut() {
                    input_device.ungrab();
                }
                input_devices = match get_input_devices(&device_filter, &ignore_filter, watch_devices) {
                    Ok(input_devices) => input_devices,
                    Err(e) => bail!("Failed to prepare input devices: {}", e),
                };
            }
            Event::ReloadConfig => match (config.modify_time, config_path.metadata().and_then(|m| m.modified())) {
                (Some(last_mtime), Ok(current_mtim)) if last_mtime == current_mtim => {
                    continue;
                }
                _ => {
                    println!("Reloading Config");
                    if let Ok(c) = load_config(&config_path) {
                        config = c;
                    }
                }
            },
        }
    }
}

enum Event {
    ReloadConfig,
    ReloadDevices,
}

fn select_readable<'a>(
    devices: impl Iterator<Item = &'a InputDevice>,
    watchers: &[&Inotify],
    timer_fd: RawFd,
) -> anyhow::Result<FdSet> {
    let mut read_fds = FdSet::new();
    read_fds.insert(timer_fd);
    for device in devices {
        read_fds.insert(device.as_raw_fd());
    }
    for inotify in watchers {
        read_fds.insert(inotify.as_raw_fd());
    }
    select(None, &mut read_fds, None, None, None)?;
    Ok(read_fds)
}
