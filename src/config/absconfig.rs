use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbsConfig {
    #[serde(default = "default_abs_info")]
    pub x: AbsInfo,
    #[serde(default = "default_abs_info")]
    pub y: AbsInfo,
    #[serde(default = "default_abs_info")]
    pub tilt_x: AbsInfo,
    #[serde(default = "default_abs_info")]
    pub tilt_y: AbsInfo,
    #[serde(default = "default_abs_info")]
    pub pressure: AbsInfo,
}

impl AbsConfig {
    pub fn new() -> AbsConfig {
        AbsConfig {
            x: default_abs_info(),
            y: default_abs_info(),
            tilt_x: default_abs_info(),
            tilt_y: default_abs_info(),
            pressure: default_abs_info(),
        }
    }
}

#[derive(Debug, Copy, Clone, Deserialize)]
pub struct AbsInfo {
    #[serde(default)]
    pub value: i32,
    #[serde(default)]
    pub min: i32,
    #[serde(default)]
    pub max: i32,
    #[serde(default)]
    pub fuzz: i32,
    #[serde(default)]
    pub flat: i32,
    #[serde(default)]
    pub resolution: i32,
}

impl AbsInfo {
    pub fn into_evdev_abs_info(self: Self) -> evdev::AbsInfo {
        evdev::AbsInfo::new(self.value, self.min, self.max, self.fuzz, self.flat, self.resolution)
    }
}

fn default_abs_info() -> AbsInfo {
    AbsInfo {
        value: 0,
        min: 0,
        max: 0,
        fuzz: 0,
        flat: 0,
        resolution: 0,
    }
}
