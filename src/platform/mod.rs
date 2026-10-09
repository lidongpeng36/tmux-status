#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("tmux-status supports Linux and macOS only");

use crate::metrics::{Battery, BatteryState, Memory};
use starship_battery::{
    Manager, State,
    units::{energy::joule, ratio::percent as Percent},
};
use sysinfo::{MemoryRefreshKind, System};

pub struct Collector {
    system: System,
    cpu_ready: bool,
}

impl Collector {
    pub fn new() -> Self {
        Self {
            system: System::new(),
            cpu_ready: false,
        }
    }

    pub fn sample(&mut self, load: bool) -> (Option<f64>, Option<Memory>, Option<[f64; 3]>) {
        // Only the metrics we display: no process, disk, GPU or network enumeration.
        self.system.refresh_cpu_usage();
        self.system
            .refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
        let cpu = self.system.global_cpu_usage() as f64;
        let cpu = (self.cpu_ready
            && !self.system.cpus().is_empty()
            && cpu.is_finite()
            && (0.0..=100.0).contains(&cpu))
        .then_some(cpu);
        self.cpu_ready = !self.system.cpus().is_empty();
        let memory = Memory {
            used_bytes: self.system.used_memory(),
            total_bytes: self.system.total_memory(),
        };
        let memory = memory.percent().map(|_| memory);
        let load = if load {
            {
                let l = System::load_average();
                let cores = self.system.cpus().len().max(1) as f64;
                let values = [l.one / cores, l.five / cores, l.fifteen / cores];
                values
                    .iter()
                    .all(|n| n.is_finite() && *n >= 0.0)
                    .then_some(values)
            }
        } else {
            None
        };
        (cpu, memory, load)
    }
}

pub fn battery() -> Result<Option<Battery>, String> {
    let manager = Manager::new().map_err(|e| e.to_string())?;
    let batteries = manager.batteries().map_err(|e| e.to_string())?;
    let mut packs = Vec::new();
    for battery in batteries {
        let b = battery.map_err(|e| e.to_string())?;
        let state = match b.state() {
            State::Charging => BatteryState::Charging,
            State::Discharging | State::Empty => BatteryState::Discharging,
            State::Full => BatteryState::Full,
            State::Paused => BatteryState::Plugged,
            _ => BatteryState::Unknown,
        };
        packs.push((
            b.state_of_charge().get::<Percent>() as f64,
            b.energy().get::<joule>() as f64,
            b.energy_full().get::<joule>() as f64,
            state,
        ));
    }
    aggregate_batteries(&packs)
}

// The library owns platform discovery and units. Our policy combines multiple
// packs by energy, falling back to a mean only when energy is unavailable.
fn aggregate_batteries(packs: &[(f64, f64, f64, BatteryState)]) -> Result<Option<Battery>, String> {
    if packs.is_empty() {
        return Ok(None);
    }
    if packs
        .iter()
        .any(|(p, _, _, _)| !p.is_finite() || !(0.0..=100.0).contains(p))
    {
        return Err("invalid battery percentage".into());
    }
    let percent = if packs
        .iter()
        .all(|(_, now, full, _)| now.is_finite() && *now >= 0.0 && full.is_finite() && *full > 0.0)
    {
        packs.iter().map(|(_, now, _, _)| now).sum::<f64>()
            / packs.iter().map(|(_, _, full, _)| full).sum::<f64>()
            * 100.0
    } else {
        packs.iter().map(|(p, _, _, _)| p).sum::<f64>() / packs.len() as f64
    }
    .clamp(0.0, 100.0);
    let first = packs[0].3;
    let state = if packs.iter().all(|(_, _, _, state)| *state == first) {
        first
    } else {
        BatteryState::Unknown
    };
    Ok(Some(Battery { percent, state }))
}

pub fn date(format: &str) -> Option<String> {
    let format = std::ffi::CString::new(format).ok()?;
    // SAFETY: localtime_r initializes tm from a valid timestamp. strftime
    // receives a bounded writable buffer and a owned NUL-terminated format.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return None;
        }
        let mut buf = [0u8; 256];
        let len = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), format.as_ptr(), &tm);
        (len > 0).then(|| String::from_utf8_lossy(&buf[..len]).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aggregation_weights_energy_and_does_not_hide_mixed_states() {
        let packs = [
            (50.0, 10.0, 20.0, BatteryState::Discharging),
            (75.0, 60.0, 80.0, BatteryState::Charging),
        ];
        let b = aggregate_batteries(&packs).unwrap().unwrap();
        assert_eq!(b.percent, 70.0);
        assert_eq!(b.state, BatteryState::Unknown);
        assert!(aggregate_batteries(&[]).unwrap().is_none());
        assert!(aggregate_batteries(&[(f64::NAN, 0.0, 0.0, BatteryState::Unknown)]).is_err());
    }
    #[test]
    fn fallback_and_zero_battery_are_distinct_from_missing() {
        let packs = [
            (0.0, 0.0, 0.0, BatteryState::Discharging),
            (80.0, 0.0, 0.0, BatteryState::Discharging),
        ];
        assert_eq!(aggregate_batteries(&packs).unwrap().unwrap().percent, 40.0);
    }
}
