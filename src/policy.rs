//! Default, conservative workload policy. Unknown focus/power never means hidden.
use std::time::{Duration, Instant};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Active,
    Background,
    Detached,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Power {
    pub battery: bool,
    pub low: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub label: &'static str,
    pub sample: u64,
    pub network: u64,
    pub paused: bool,
}
impl Plan {
    pub fn fixed(sample: u64, network: u64) -> Self {
        Self {
            label: "fixed",
            sample,
            network,
            paused: false,
        }
    }
}
pub struct Policy {
    quiet: Option<(Activity, Instant)>,
}
impl Policy {
    pub fn new() -> Self {
        Self { quiet: None }
    }
    pub fn evaluate(
        &mut self,
        now: Instant,
        activity: Activity,
        power: Power,
        sample: u64,
        network: u64,
    ) -> Plan {
        let mut p = Plan {
            label: "active",
            sample,
            network,
            paused: false,
        };
        if power.battery {
            p.label = "battery";
            p.sample = p.sample.max(10);
            p.network = p.network.max(60);
        }
        if power.low {
            p.label = "low-power";
            p.sample = p.sample.max(30);
            p.network = p.network.max(300);
        }
        if activity == Activity::Active {
            self.quiet = None;
        } else {
            if self.quiet.is_none_or(|(prior, _)| prior != activity) {
                self.quiet = Some((activity, now));
            }
            if now.duration_since(self.quiet.unwrap().1) >= Duration::from_secs(30) {
                if activity == Activity::Detached {
                    p.label = "paused";
                    p.paused = true;
                    p.sample = 30;
                    p.network = 0;
                } else {
                    p.label = "background";
                    p.sample = p.sample.max(30);
                    p.network = p.network.max(300);
                }
            }
        }
        p
    }
    pub fn transition_due(&self) -> Option<Instant> {
        self.quiet.map(|(_, at)| at + Duration::from_secs(30))
    }
}
pub fn activity(text: &str) -> Activity {
    let mut attached = false;
    for line in text.lines().filter(|s| !s.is_empty()) {
        attached = true;
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 3 {
            return Activity::Active;
        }
        let flags: Vec<_> = fields[0].split(',').collect();
        if flags.contains(&"focused") {
            return Activity::Active;
        }
        if fields[2] != "1" || !fields[1].split(',').any(|s| s == "focus") {
            return Activity::Active;
        }
    }
    if attached {
        Activity::Background
    } else {
        Activity::Detached
    }
}
pub struct PowerReader {
    last: Option<(Instant, Power)>,
}
impl PowerReader {
    pub fn new() -> Self {
        Self { last: None }
    }
    pub fn get(&mut self, force: bool) -> Power {
        if !force
            && let Some((at, p)) = self.last
            && at.elapsed() < Duration::from_secs(30)
        {
            return p;
        }
        let p = read_power();
        self.last = Some((Instant::now(), p));
        p
    }
}
#[cfg(target_os = "macos")]
fn read_power() -> Power {
    use core_foundation_sys::{
        base::{CFRelease, CFTypeRef},
        string::{CFStringGetCString, CFStringRef, kCFStringEncodingUTF8},
    };
    use objc2::{class, msg_send, runtime::AnyObject, sel};
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
        fn IOPSGetProvidingPowerSourceType(blob: CFTypeRef) -> CFStringRef;
    }
    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {}
    // Public APIs only; respondsToSelector keeps the macOS 11 build compatible.
    unsafe {
        let mut p = Power::default();
        let blob = IOPSCopyPowerSourcesInfo();
        if !blob.is_null() {
            let source = IOPSGetProvidingPowerSourceType(blob);
            let mut bytes = [0i8; 128];
            if !source.is_null()
                && CFStringGetCString(
                    source,
                    bytes.as_mut_ptr(),
                    bytes.len() as isize,
                    kCFStringEncodingUTF8,
                ) != 0
            {
                p.battery = std::ffi::CStr::from_ptr(bytes.as_ptr()).to_bytes() == b"Battery Power";
            }
            CFRelease(blob);
        }
        let info: *mut AnyObject = msg_send![class!(NSProcessInfo), processInfo];
        let supported: bool = msg_send![info,respondsToSelector:sel!(isLowPowerModeEnabled)];
        if supported {
            p.low = msg_send![info, isLowPowerModeEnabled];
        }
        p
    }
}
#[cfg(target_os = "linux")]
fn read_power() -> Power {
    use std::{fs, path::Path};
    let mut p = Power::default();
    let mut ac = false;
    if let Ok(entries) = fs::read_dir("/sys/class/power_supply") {
        for entry in entries.flatten() {
            let path = entry.path();
            let kind = fs::read_to_string(path.join("type")).unwrap_or_default();
            if kind.trim() == "Battery"
                && fs::read_to_string(path.join("status"))
                    .unwrap_or_default()
                    .trim()
                    == "Discharging"
            {
                p.battery = true;
            }
            if matches!(kind.trim(), "Mains" | "USB" | "USB_C")
                && fs::read_to_string(path.join("online"))
                    .unwrap_or_default()
                    .trim()
                    == "1"
            {
                ac = true;
            }
        }
    }
    if ac {
        p.battery = false;
    }
    p.low = fs::read_to_string(Path::new("/sys/firmware/acpi/platform_profile"))
        .unwrap_or_default()
        .trim()
        == "low-power";
    p
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conservative_focus() {
        assert_eq!(activity(""), Activity::Detached);
        assert_eq!(activity("attached\tfocus,RGB\t1"), Activity::Background);
        assert_eq!(activity("attached\tRGB\t1"), Activity::Active);
        assert_eq!(activity("attached\tfocus\t0"), Activity::Active);
        assert_eq!(
            activity("attached\tfocus\t1\nattached,focused\tfocus\t1"),
            Activity::Active
        );
    }
    #[test]
    fn defaults_grace_and_resume() {
        let now = Instant::now();
        let mut p = Policy::new();
        assert_eq!(
            p.evaluate(now, Activity::Active, Power::default(), 5, 30)
                .sample,
            5
        );
        assert_eq!(
            p.evaluate(
                now,
                Activity::Active,
                Power {
                    battery: true,
                    low: false
                },
                5,
                30
            )
            .network,
            60
        );
        assert_eq!(
            p.evaluate(
                now,
                Activity::Active,
                Power {
                    battery: false,
                    low: true
                },
                5,
                30
            )
            .sample,
            30
        );
        assert!(
            !p.evaluate(now, Activity::Detached, Power::default(), 5, 30)
                .paused
        );
        assert!(
            !p.evaluate(
                now + Duration::from_secs(29),
                Activity::Detached,
                Power::default(),
                5,
                30
            )
            .paused
        );
        assert!(
            p.evaluate(
                now + Duration::from_secs(30),
                Activity::Detached,
                Power::default(),
                5,
                30
            )
            .paused
        );
        assert_eq!(
            p.evaluate(
                now + Duration::from_secs(31),
                Activity::Active,
                Power::default(),
                5,
                30
            )
            .sample,
            5
        );
        assert_eq!(
            p.evaluate(
                now + Duration::from_secs(32),
                Activity::Background,
                Power::default(),
                5,
                30
            )
            .sample,
            5
        );
        assert_eq!(
            p.evaluate(
                now + Duration::from_secs(62),
                Activity::Background,
                Power::default(),
                5,
                30
            )
            .sample,
            30
        );
    }
    #[test]
    fn slower_user_intervals_are_preserved() {
        let mut p = Policy::new();
        assert_eq!(
            p.evaluate(
                Instant::now(),
                Activity::Active,
                Power {
                    battery: true,
                    low: true
                },
                120,
                600
            )
            .sample,
            120
        );
        assert_eq!(
            p.evaluate(
                Instant::now(),
                Activity::Active,
                Power {
                    battery: true,
                    low: true
                },
                120,
                600
            )
            .network,
            600
        );
    }
}
