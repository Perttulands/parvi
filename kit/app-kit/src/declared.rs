//! The settings a page declares beside its page ([`crate::Page::SETTINGS`]):
//! what the shell's Settings page lists and changes, each kept under its
//! key in the page's section of `app.json`. The page reads its section as
//! before and hears [`crate::Page::settings_changed`] when one changes.

use serde_json::Value;

/// One setting: where it is kept, what it is called, and how it is set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Setting {
    /// Its key in the page's section.
    pub key: &'static str,
    pub label: &'static str,
    /// A line saying what it does; empty for none.
    pub note: &'static str,
    pub control: Control,
}

/// How a setting is set, with its value while none is kept.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Control {
    Toggle {
        default: bool,
    },
    /// Stepped between `min` and `max`.
    Number {
        default: f64,
        min: f64,
        max: f64,
        step: f64,
    },
    Text {
        default: &'static str,
    },
}

impl Setting {
    pub const fn toggle(key: &'static str, label: &'static str, default: bool) -> Self {
        Self {
            key,
            label,
            note: "",
            control: Control::Toggle { default },
        }
    }

    pub const fn number(
        key: &'static str,
        label: &'static str,
        default: f64,
        (min, max, step): (f64, f64, f64),
    ) -> Self {
        Self {
            key,
            label,
            note: "",
            control: Control::Number {
                default,
                min,
                max,
                step,
            },
        }
    }

    pub const fn text(key: &'static str, label: &'static str, default: &'static str) -> Self {
        Self {
            key,
            label,
            note: "",
            control: Control::Text { default },
        }
    }

    pub const fn note(mut self, note: &'static str) -> Self {
        self.note = note;
        self
    }

    /// Its value while none is kept.
    pub fn default_value(&self) -> Value {
        match self.control {
            Control::Toggle { default } => Value::from(default),
            Control::Number { default, .. } => Value::from(default),
            Control::Text { default } => Value::from(default),
        }
    }

    /// What `kept` holds for it, or its default where it holds nothing of
    /// the setting's kind.
    pub fn read(&self, kept: Option<&Value>) -> Value {
        let fits = matches!(
            (self.control, kept),
            (Control::Toggle { .. }, Some(Value::Bool(_)))
                | (Control::Number { .. }, Some(Value::Number(_)))
                | (Control::Text { .. }, Some(Value::String(_)))
        );
        match kept {
            Some(v) if fits => v.clone(),
            _ => self.default_value(),
        }
    }

    /// A number `steps` steps from `value`, kept within its bounds.
    pub fn stepped(&self, value: &Value, steps: f64) -> Value {
        let Control::Number { min, max, step, .. } = self.control else {
            return value.clone();
        };
        let now = value.as_f64().unwrap_or(min);
        Value::from((now + steps * step).clamp(min, max))
    }
}

/// The first key `settings` declares twice, if any.
pub fn twice(settings: &[Setting]) -> Option<&'static str> {
    settings
        .iter()
        .enumerate()
        .find(|(i, a)| settings[i + 1..].iter().any(|b| b.key == a.key))
        .map(|(_, a)| a.key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setting_reads_its_kind_or_its_default_and_steps_within_bounds() {
        let size = Setting::number("size", "Size", 14.0, (8.0, 16.0, 1.0));
        assert_eq!(size.read(None), Value::from(14.0));
        assert_eq!(size.read(Some(&Value::from("big"))), Value::from(14.0));
        assert_eq!(size.read(Some(&Value::from(10))), Value::from(10));
        assert_eq!(size.stepped(&Value::from(15.5), 1.0), Value::from(16.0));
        assert_eq!(size.stepped(&Value::from(8.0), -1.0), Value::from(8.0));
        let loud = Setting::toggle("loud", "Loud", true);
        assert_eq!(loud.read(Some(&Value::from(false))), Value::from(false));
        assert_eq!(twice(&[loud, size, loud]), Some("loud"));
        assert_eq!(twice(&[loud, size]), None);
    }
}
