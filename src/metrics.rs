use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Memory {
    pub used_bytes: u64,
    pub total_bytes: u64,
}

impl Memory {
    pub fn percent(self) -> Option<f64> {
        (self.total_bytes > 0 && self.used_bytes <= self.total_bytes)
            .then(|| self.used_bytes as f64 / self.total_bytes as f64 * 100.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BatteryState {
    Charging,
    Discharging,
    Full,
    Plugged,
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Battery {
    pub percent: f64,
    pub state: BatteryState,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reachability {
    Reachable,
    Unreachable,
    Unknown,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub cpu_percent: Option<f64>,
    pub memory: Option<Memory>,
    pub load_per_core: Option<[f64; 3]>,
    pub battery: Option<Battery>,
    pub battery_error: bool,
    pub reachability: Option<Reachability>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_memory_is_unknown() {
        assert_eq!(
            Memory {
                used_bytes: 1,
                total_bytes: 0
            }
            .percent(),
            None
        );
        assert_eq!(
            Memory {
                used_bytes: 2,
                total_bytes: 1
            }
            .percent(),
            None
        );
    }
}
