use crate::{
    Args,
    appearance::{Labels, interpolate},
    metrics::{BatteryState, Reachability, Snapshot},
    platform,
};

fn styled(text: &str, color: &str, plain: bool) -> String {
    if plain {
        text.into()
    } else {
        format!("#[fg={color}]{text}#[default]")
    }
}

fn size(bytes: u64, args: &Args) -> String {
    let style = &args.appearance;
    let divisor = match style.size_unit.as_str() {
        "M" => 1048576.0,
        "K" => 1024.0,
        _ => 1073741824.0,
    };
    format!(
        "{:.*}{}",
        style.size_precision,
        bytes as f64 / divisor,
        style.size_unit
    )
}

fn segment(name: &str, snapshot: &Snapshot, args: &Args) -> Option<String> {
    let a = &args.appearance;
    let text_labels = a.labels == Labels::Text;
    match name {
        "cpu" => {
            let label = if args.plain || (text_labels && a.cpu_label == " ") {
                "CPU:"
            } else {
                &a.cpu_label
            };
            let percent = snapshot
                .cpu_percent
                .map(|n| format!("{:.*}%", a.cpu_precision, n))
                .unwrap_or_else(|| "--".into());
            let color = snapshot
                .cpu_percent
                .map(|n| a.color(n, a.cpu_medium, a.cpu_high))
                .unwrap_or(&a.unknown_color);
            Some(interpolate(
                &a.cpu_template,
                &[
                    ("label", label.into()),
                    ("value", styled(&percent, color, args.plain)),
                    ("percent", percent),
                ],
            ))
        }
        "memory" => {
            let label = if args.plain || (text_labels && a.mem_label == "󰍛 ") {
                "MEM:"
            } else {
                &a.mem_label
            };
            let values = snapshot.memory.and_then(|m| m.percent().map(|p| (m, p)));
            let percent = values
                .map(|(_, p)| format!("{:.*}%", a.mem_precision, p))
                .unwrap_or_else(|| "--".into());
            let color = values
                .map(|(_, p)| a.color(p, a.mem_medium, a.mem_high))
                .unwrap_or(&a.unknown_color);
            if let Some((m, _)) = values {
                Some(interpolate(
                    &a.mem_template,
                    &[
                        ("label", label.into()),
                        ("value", styled(&percent, color, args.plain)),
                        ("percent", percent),
                        ("used", size(m.used_bytes, args)),
                        ("free", size(m.total_bytes - m.used_bytes, args)),
                        ("total", size(m.total_bytes, args)),
                    ],
                ))
            } else {
                Some(format!("{label}--"))
            }
        }
        "load" => Some(
            snapshot
                .load_per_core
                .map(|[x, y, z]| format!("{x:.2} {y:.2} {z:.2}"))
                .unwrap_or_else(|| "LOAD:--".into()),
        ),
        "date" => {
            platform::date(&a.date_format).map(|date| styled(&date, &a.date_color, args.plain))
        }
        "battery" => {
            let b = match snapshot.battery {
                Some(b) => b,
                None => return snapshot.battery_error.then(|| "BAT:--".into()),
            };
            let icon = if args.plain {
                "BAT"
            } else {
                match b.state {
                    BatteryState::Charging => &a.charging_icon,
                    BatteryState::Full => &a.full_icon,
                    BatteryState::Plugged => &a.plugged_icon,
                    BatteryState::Unknown => &a.battery_unknown_icon,
                    BatteryState::Discharging => {
                        let index = a
                            .battery_upper_bounds
                            .iter()
                            .position(|bound| b.percent <= *bound)
                            .unwrap_or(a.battery_icons.len() - 1);
                        &a.battery_icons[index]
                    }
                }
            };
            let color = match b.state {
                BatteryState::Charging | BatteryState::Full => &a.charging_color,
                _ if b.percent <= a.battery_low => &a.critical_color,
                _ if b.percent <= a.battery_medium => &a.warning_color,
                _ => &a.normal_color,
            };
            let state = match b.state {
                BatteryState::Charging => "charging",
                BatteryState::Full => "full",
                BatteryState::Plugged => "plugged",
                BatteryState::Unknown => "unknown",
                BatteryState::Discharging => "discharging",
            };
            Some(styled(
                &interpolate(
                    &a.battery_template,
                    &[
                        ("icon", icon.into()),
                        ("percent", format!("{:.0}%", b.percent)),
                        ("state", state.into()),
                    ],
                ),
                color,
                args.plain,
            ))
        }
        "network" => snapshot.reachability.map(|state| {
            let (text, icon, color) = match state {
                Reachability::Reachable => ("up", &a.online_icon, &a.normal_color),
                Reachability::Unreachable => ("down", &a.offline_icon, &a.critical_color),
                Reachability::Limited => ("limited", &a.limited_icon, &a.warning_color),
                Reachability::Unknown => ("--", &a.network_unknown_icon, &a.unknown_color),
            };
            let text = if args.plain {
                format!("{}:{text}", if args.network { "NET" } else { "TCP" })
            } else {
                icon.clone()
            };
            styled(&text, color, args.plain)
        }),
        _ => None,
    }
}

pub fn profile(snapshot: &Snapshot, args: &Args, remote: bool) -> String {
    let segments = if remote {
        &args.appearance.remote_segments
    } else {
        &args.appearance.local_segments
    };
    segments
        .iter()
        .filter_map(|name| segment(name, snapshot, args))
        .collect::<Vec<_>>()
        .join(if args.plain {
            " | "
        } else {
            &args.appearance.separator
        })
}

pub fn render(snapshot: &Snapshot, args: &Args) -> String {
    let mut local = args.clone();
    local
        .appearance
        .local_segments
        .retain(|s| !(args.no_date && s == "date" || args.no_battery && s == "battery"));
    if args.load && !local.appearance.local_segments.iter().any(|s| s == "load") {
        let index = local
            .appearance
            .local_segments
            .iter()
            .position(|s| s == "memory")
            .map_or(0, |i| i + 1);
        local.appearance.local_segments.insert(index, "load".into());
    }
    profile(snapshot, &local, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Memory;
    use clap::Parser;
    fn snapshot() -> Snapshot {
        Snapshot {
            cpu_percent: None,
            memory: None,
            load_per_core: None,
            battery: None,
            battery_error: false,
            reachability: None,
        }
    }
    #[test]
    fn unknown_is_not_zero_and_plain_remains_text() {
        let args = Args::parse_from(["tmux-status", "--plain", "--no-date"]);
        let mut s = snapshot();
        assert_eq!(render(&s, &args), "CPU:-- | MEM:--");
        s.battery_error = true;
        assert!(render(&s, &args).ends_with("BAT:--"));
    }
    #[test]
    fn default_icons_and_partial_override_preserve_percent() {
        let mut args = Args::parse_from(["tmux-status", "--no-date"]);
        let mut s = snapshot();
        s.cpu_percent = Some(12.5);
        s.memory = Some(Memory {
            used_bytes: 1073741824,
            total_bytes: 2147483648,
        });
        let output = render(&s, &args);
        assert!(output.contains(" "));
        assert!(output.contains("󰍛 "));
        assert!(output.contains("12.5%#[default]"));
        args.appearance=crate::appearance::Appearance::load(None,r#"{"cpu_label":"C: ","mem_template":"{label}{used}/{total}","separator":" / ","local_segments":["memory","cpu"]}"#).unwrap();
        let output = render(&s, &args);
        assert!(output.starts_with("󰍛 1.0G/2.0G / C: "));
        args.appearance.labels = Labels::Text;
        assert!(render(&s, &args).contains("C: "));
        args.appearance.cpu_label = " ".into();
        assert!(render(&s, &args).contains("CPU:"));
    }
}
