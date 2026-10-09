use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, path::Path};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Labels {
    #[default]
    Icons,
    Text,
}

/// All defaults live here, rather than being copied into each tmux.conf.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    pub labels: Labels,
    pub cpu_label: String,
    pub mem_label: String,
    pub cpu_label_color: String,
    pub mem_label_color: String,
    pub cpu_width: usize,
    pub mem_width: usize,
    pub size_width: usize,
    pub cpu_template: String,
    pub mem_template: String,
    pub cpu_precision: usize,
    pub mem_precision: usize,
    pub size_precision: usize,
    pub size_unit: String,
    pub cpu_medium: f64,
    pub cpu_high: f64,
    pub mem_medium: f64,
    pub mem_high: f64,
    pub normal_color: String,
    pub warning_color: String,
    pub critical_color: String,
    pub unknown_color: String,
    pub date_color: String,
    pub charging_color: String,
    pub date_format: String,
    pub separator: String,
    pub local_segments: Vec<String>,
    pub remote_segments: Vec<String>,
    pub battery_template: String,
    pub battery_icons: Vec<String>,
    pub battery_upper_bounds: Vec<f64>,
    pub charging_icon: String,
    pub full_icon: String,
    pub plugged_icon: String,
    pub battery_unknown_icon: String,
    pub battery_low: f64,
    pub battery_medium: f64,
    pub online_icon: String,
    pub offline_icon: String,
    pub limited_icon: String,
    pub network_unknown_icon: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            labels: Labels::Icons,
            cpu_label: "  ".into(),
            mem_label: "  ".into(),
            cpu_label_color: "#61afef".into(),
            mem_label_color: "#c678dd".into(),
            cpu_width: 6,
            mem_width: 4,
            size_width: 6,
            cpu_template: "{label}{value}".into(),
            mem_template: "{label}{value} {used}".into(),
            cpu_precision: 1,
            mem_precision: 0,
            size_precision: 1,
            size_unit: "G".into(),
            cpu_medium: 30.0,
            cpu_high: 80.0,
            mem_medium: 75.0,
            mem_high: 90.0,
            normal_color: "colour076".into(),
            warning_color: "colour220".into(),
            critical_color: "colour160".into(),
            unknown_color: "colour245".into(),
            date_color: "colour134".into(),
            charging_color: "#3daee9".into(),
            date_format: "%b %d %H:%M".into(),
            separator: "  ".into(),
            local_segments: ["cpu", "memory", "date", "battery", "network"]
                .map(String::from)
                .to_vec(),
            remote_segments: ["cpu", "memory", "load", "network"]
                .map(String::from)
                .to_vec(),
            battery_template: "{icon} {percent}".into(),
            battery_icons: ["󰁺", "󰁻", "󰁼", "󰁽", "󰁾", "󰁿", "󰂀", "󰂂"]
                .map(String::from)
                .to_vec(),
            battery_upper_bounds: vec![12.5, 25.0, 37.5, 50.0, 62.5, 75.0, 87.5, 100.0],
            charging_icon: "󰂄".into(),
            full_icon: "󰂅".into(),
            plugged_icon: "".into(),
            battery_unknown_icon: "󰂃".into(),
            battery_low: 15.0,
            battery_medium: 50.0,
            online_icon: "●".into(),
            offline_icon: "●".into(),
            limited_icon: "●".into(),
            network_unknown_icon: "○".into(),
        }
    }
}

impl Appearance {
    pub fn load(file: Option<&Path>, inline: &str) -> Result<Self, String> {
        let mut values = if let Some(path) = file {
            serde_json::from_str::<serde_json::Value>(
                &fs::read_to_string(path).map_err(|e| format!("appearance file: {e}"))?,
            )
            .map_err(|e| e.to_string())?
        } else {
            serde_json::json!({})
        };
        let overrides: serde_json::Value =
            serde_json::from_str(inline).map_err(|e| e.to_string())?;
        let object = values
            .as_object_mut()
            .ok_or("appearance must be a JSON object")?;
        for (key, value) in overrides
            .as_object()
            .ok_or("appearance must be a JSON object")?
        {
            object.insert(key.clone(), value.clone());
        }
        let result: Self = serde_json::from_value(values).map_err(|e| e.to_string())?;
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<(), String> {
        for (name, medium, high) in [
            ("cpu", self.cpu_medium, self.cpu_high),
            ("memory", self.mem_medium, self.mem_high),
            ("battery", self.battery_low, self.battery_medium),
        ] {
            if !medium.is_finite()
                || !high.is_finite()
                || !(0.0..=100.0).contains(&medium)
                || !(medium..=100.0).contains(&high)
            {
                return Err(format!("invalid {name} thresholds"));
            }
        }
        if self.cpu_precision > 3 || self.mem_precision > 3 || self.size_precision > 3 {
            return Err("precision must be 0..3".into());
        }
        if [self.cpu_width, self.mem_width, self.size_width]
            .iter()
            .any(|w| *w > 64)
        {
            return Err("numeric widths must be 0..64".into());
        }
        if !["G", "M", "K"].contains(&self.size_unit.as_str()) {
            return Err("size_unit must be G, M or K (binary units)".into());
        }
        if self.battery_icons.is_empty()
            || self.battery_icons.len() != self.battery_upper_bounds.len()
            || self.battery_upper_bounds.last() != Some(&100.0)
            || self
                .battery_upper_bounds
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=100.0).contains(v))
            || self.battery_upper_bounds.windows(2).any(|v| v[0] >= v[1])
        {
            return Err(
                "battery tiers require ordered bounds ending at 100 and one icon per bound".into(),
            );
        }
        for segments in [&self.local_segments, &self.remote_segments] {
            let mut seen = HashSet::new();
            for segment in segments {
                if !["cpu", "memory", "load", "date", "battery", "network"]
                    .contains(&segment.as_str())
                    || !seen.insert(segment)
                {
                    return Err(format!("unknown or repeated segment: {segment}"));
                }
            }
        }
        // Keep one complete output line. Colors and templates are user-owned
        // tmux markup, but terminal/control characters are never accepted.
        let values = serde_json::to_value(self).map_err(|e| e.to_string())?;
        fn check(value: &serde_json::Value) -> bool {
            match value {
                serde_json::Value::String(s) => !s.chars().any(char::is_control),
                serde_json::Value::Array(a) => a.iter().all(check),
                serde_json::Value::Object(o) => o.values().all(check),
                _ => true,
            }
        }
        if !check(&values) {
            return Err("appearance text must not contain control characters".into());
        }
        for color in [
            &self.normal_color,
            &self.warning_color,
            &self.critical_color,
            &self.unknown_color,
            &self.date_color,
            &self.cpu_label_color,
            &self.mem_label_color,
            &self.charging_color,
        ] {
            if color.is_empty() || !color.chars().all(|c| c.is_ascii_alphanumeric() || c == '#') {
                return Err(format!("invalid tmux foreground color: {color}"));
            }
        }
        template(&self.cpu_template, &["label", "value", "percent"])?;
        template(
            &self.mem_template,
            &["label", "value", "percent", "used", "free", "total"],
        )?;
        template(&self.battery_template, &["icon", "percent", "state"])?;
        Ok(())
    }

    pub fn color(&self, value: f64, medium: f64, high: f64) -> &str {
        if value >= high {
            &self.critical_color
        } else if value >= medium {
            &self.warning_color
        } else {
            &self.normal_color
        }
    }
}

fn template(input: &str, allowed: &[&str]) -> Result<(), String> {
    let mut rest = input;
    while let Some(start) = rest.find('{') {
        let tail = &rest[start + 1..];
        let end = tail.find('}').ok_or("unclosed template placeholder")?;
        if !allowed.contains(&&tail[..end]) {
            return Err(format!("unknown template placeholder: {}", &tail[..end]));
        }
        rest = &tail[end + 1..];
    }
    Ok(())
}

pub fn interpolate(template: &str, replacements: &[(&str, String)]) -> String {
    // One pass avoids interpreting braces in a user's label as another token.
    let mut result = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        result.push_str(&rest[..start]);
        let tail = &rest[start + 1..];
        if let Some(end) = tail.find('}') {
            let key = &tail[..end];
            if let Some((_, value)) = replacements.iter().find(|(name, _)| *name == key) {
                result.push_str(value);
            }
            rest = &tail[end + 1..];
        } else {
            result.push_str(&rest[start..]);
            rest = "";
            break;
        }
    }
    result.push_str(rest);
    result
}

/// Compact IPC preserves defaults and stays below small macOS datagram limits.
pub fn serialize_overrides<S: serde::Serializer>(
    style: &Appearance,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut value = serde_json::to_value(style).map_err(serde::ser::Error::custom)?;
    let defaults =
        serde_json::to_value(Appearance::default()).map_err(serde::ser::Error::custom)?;
    if let (Some(values), Some(defaults)) = (value.as_object_mut(), defaults.as_object()) {
        values.retain(|key, value| defaults.get(key) != Some(value));
    }
    value.serialize(serializer)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_override_keeps_other_defaults_and_rejects_typos() {
        let style =
            Appearance::load(None, r##"{"cpu_label":"C: ","normal_color":"#abcdef"}"##).unwrap();
        assert_eq!(style.cpu_label, "C: ");
        assert_eq!(style.mem_label, "  ");
        assert_eq!(style.cpu_high, 80.0);
        assert!(Appearance::load(None, r#"{"cpu_labl":"oops"}"#).is_err());
        assert!(Appearance::load(None, r#"{"cpu_medium":90,"cpu_high":80}"#).is_err());
        assert!(Appearance::load(None, r#"{"cpu_template":"{wrong}"}"#).is_err());
    }
}
