use crate::{
    Args,
    metrics::{BatteryState, Reachability, Snapshot},
    platform,
};

fn color(value: f64, medium: f64, stress: f64) -> &'static str {
    if value >= stress {
        "colour160"
    } else if value >= medium {
        "colour220"
    } else {
        "colour076"
    }
}

fn styled(text: &str, color: &str, plain: bool) -> String {
    if plain {
        text.into()
    } else {
        format!("#[fg={color}]{text}#[default]")
    }
}

pub fn render(snapshot: &Snapshot, args: &Args) -> String {
    let cpu = match snapshot.cpu_percent {
        Some(n) => format!(
            "CPU:{}",
            styled(&format!("{n:.1}%"), color(n, 30.0, 80.0), args.plain)
        ),
        None => "CPU:--".into(),
    };
    let memory = match snapshot.memory.and_then(|m| m.percent().map(|p| (m, p))) {
        Some((m, p)) => format!(
            "MEM:{} {:.1}G",
            styled(&format!("{p:.0}%"), color(p, 75.0, 90.0), args.plain),
            m.used_bytes as f64 / 1073741824.0
        ),
        None => "MEM:--".into(),
    };
    let mut parts = vec![cpu, memory];
    if args.load {
        parts.push(match snapshot.load_per_core {
            Some([a, b, c]) => format!("{a:.2} {b:.2} {c:.2}"),
            None => "LOAD:--".into(),
        });
    }
    if !args.no_date
        && let Some(date) = platform::date()
    {
        parts.push(styled(&date, "colour134", args.plain));
    }
    if !args.no_battery {
        if let Some(b) = snapshot.battery {
            let icon = if args.plain {
                "BAT"
            } else {
                match b.state {
                    BatteryState::Charging => "󰂄",
                    BatteryState::Full => "󰂅",
                    BatteryState::Plugged => "",
                    BatteryState::Unknown => "󰂃",
                    BatteryState::Discharging => {
                        const ICONS: [&str; 8] = ["󰁺", "󰁻", "󰁼", "󰁽", "󰁾", "󰁿", "󰂀", "󰂂"];
                        ICONS[((b.percent / 12.5).ceil() as usize)
                            .saturating_sub(1)
                            .min(7)]
                    }
                }
            };
            let fg = match b.state {
                BatteryState::Charging | BatteryState::Full => "#3daee9",
                _ if b.percent <= 15.0 => "colour160",
                _ if b.percent <= 50.0 => "colour220",
                _ => "colour076",
            };
            parts.push(styled(&format!("{icon} {:.0}%", b.percent), fg, args.plain));
        } else if snapshot.battery_error {
            parts.push("BAT:--".into());
        }
    }
    if let Some(state) = snapshot.reachability {
        let (text, fg) = match state {
            Reachability::Reachable => ("TCP:up", "colour076"),
            Reachability::Unreachable => ("TCP:down", "colour160"),
            Reachability::Unknown => ("TCP:--", "colour245"),
            Reachability::Limited => ("NET:limited", "colour220"),
        };
        let text = if args.network {
            text.replace("TCP:", "NET:")
        } else {
            text.into()
        };
        let icon = if state == Reachability::Unknown {
            "○"
        } else {
            "●"
        };
        parts.push(styled(
            if args.plain { &text } else { icon },
            fg,
            args.plain,
        ));
    }
    parts.join(if args.plain { " | " } else { "  " })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn missing_metrics_are_not_zero_and_no_battery_is_hidden() {
        let args = Args::parse_from(["tmux-status", "--plain", "--no-date"]);
        let mut snapshot = Snapshot {
            cpu_percent: None,
            memory: None,
            load_per_core: None,
            battery: None,
            battery_error: false,
            reachability: None,
        };
        assert_eq!(render(&snapshot, &args), "CPU:-- | MEM:--");
        snapshot.battery_error = true;
        assert!(render(&snapshot, &args).ends_with("BAT:--"));
    }
}
