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

fn padded(text: &str, width: usize, plain: bool) -> String {
    if plain {
        text.into()
    } else {
        format!("{text:>width$}")
    }
}

fn size(bytes: u64, args: &Args) -> String {
    let style = &args.appearance;
    let divisor = match style.size_unit.as_str() {
        "M" => 1048576.0,
        "K" => 1024.0,
        _ => 1073741824.0,
    };
    let value = format!(
        "{:.*}{}",
        style.size_precision,
        bytes as f64 / divisor,
        style.size_unit
    );
    padded(&value, style.size_width, args.plain)
}

fn segment(name: &str, snapshot: &Snapshot, args: &Args) -> Option<String> {
    let a = &args.appearance;
    let text_labels = a.labels == Labels::Text;
    match name {
        "cpu" => {
            let label = if args.plain || (text_labels && a.cpu_label == "  ") {
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
                    ("label", styled(label, &a.cpu_label_color, args.plain)),
                    (
                        "value",
                        styled(
                            &padded(&percent, a.cpu_width, args.plain),
                            color,
                            args.plain,
                        ),
                    ),
                    ("percent", percent),
                ],
            ))
        }
        "memory" => {
            let label = if args.plain || (text_labels && a.mem_label == "  ") {
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
                        ("label", styled(label, &a.mem_label_color, args.plain)),
                        (
                            "value",
                            styled(
                                &padded(&percent, a.mem_width, args.plain),
                                color,
                                args.plain,
                            ),
                        ),
                        ("percent", percent),
                        ("used", size(m.used_bytes, args)),
                        ("free", size(m.total_bytes - m.used_bytes, args)),
                        ("total", size(m.total_bytes, args)),
                    ],
                ))
            } else {
                Some(format!(
                    "{}{}",
                    styled(label, &a.mem_label_color, args.plain),
                    padded("--", a.mem_width, args.plain)
                ))
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
    fn percentage_digit_transitions_keep_icon_columns() {
        let mut args = Args::parse_from(["tmux-status", "--no-date"]);
        args.appearance.local_segments = vec!["cpu".into(), "memory".into()];
        let mut s = snapshot();
        s.memory = Some(Memory {
            used_bytes: 9 * 1073741824,
            total_bytes: 100 * 1073741824,
        });
        let strip = |text: &str| {
            let mut rest = text;
            let mut visible = String::new();
            while let Some(start) = rest.find("#[") {
                visible.push_str(&rest[..start]);
                let tail = &rest[start + 2..];
                rest = &tail[tail.find(']').unwrap() + 1..];
            }
            visible.push_str(rest);
            visible
        };
        let mut baseline = None;
        for cpu in [
            None,
            Some(0.0),
            Some(9.9),
            Some(10.0),
            Some(99.9),
            Some(100.0),
        ] {
            s.cpu_percent = cpu;
            for used in [9, 10, 99, 100] {
                s.memory.as_mut().unwrap().used_bytes = used * 1073741824;
                let text = strip(&render(&s, &args));
                let columns = (
                    text.chars().count(),
                    text.chars().position(|c| c == '').unwrap(),
                );
                assert_eq!(*baseline.get_or_insert(columns), columns, "{text}");
            }
        }
        s.cpu_percent = Some(9.9);
        assert!(strip(&render(&s, &args)).starts_with("    9.9%"));
        args.appearance.cpu_width = 0;
        assert!(strip(&render(&s, &args)).starts_with("  9.9%"));
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
        assert!(output.contains(" "));
        assert!(output.contains(" "));
        assert!(output.contains("12.5%#[default]"));
        args.appearance=crate::appearance::Appearance::load(None,r#"{"cpu_label":"C: ","mem_template":"{label}{used}/{total}","separator":" / ","local_segments":["memory","cpu"]}"#).unwrap();
        let output = render(&s, &args);
        assert!(output.contains("  #[default]  1.0G/  2.0G / #[fg=#61afef]C: "));
        args.appearance.labels = Labels::Text;
        assert!(render(&s, &args).contains("C: "));
        args.appearance.cpu_label = "  ".into();
        assert!(render(&s, &args).contains("CPU:"));
    }
}
