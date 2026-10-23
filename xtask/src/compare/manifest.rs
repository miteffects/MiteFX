//! Our plugins' parameters by id, from their frozen manifests (`tests/golden/<plugin>.params.txt`),
//! so a case sets each one at its exact normalized value and host index (the manifest's order is
//! the host order, tested in each plugin). The normalization is nih-plug's, as `mite-plugin`
//! builds the parameters: linear, a power-law skew for log ranges, index / (n − 1) for choices.

use std::path::Path;

use super::reaper::Value;

enum Kind {
    Float { min: f64, max: f64, log: bool },
    Choice(Vec<String>),
    Bool,
}

pub struct Manifest {
    entries: Vec<(String, Kind)>,
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut entries = Vec::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let f: Vec<&str> = line.split(" | ").collect();
            let kind = match f.get(1).copied() {
                Some("bool") => Kind::Bool,
                Some("choice") => Kind::Choice(f[2].split(", ").map(str::to_owned).collect()),
                _ => {
                    let n = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).ok_or_else(|| format!("bad manifest line: {line}"));
                    Kind::Float { min: n(2)?, max: n(3)?, log: f.get(5) == Some(&"Log") }
                }
            };
            entries.push((f[0].to_owned(), kind));
        }
        Ok(Self { entries })
    }

    fn find(&self, id: &str) -> (usize, &Kind) {
        let i = self.entries.iter().position(|(k, _)| k == id).unwrap_or_else(|| panic!("no parameter {id} in the manifest"));
        (i, &self.entries[i].1)
    }

    pub fn num(&self, id: &str, v: f64) -> (String, Value) {
        let (i, kind) = self.find(id);
        let Kind::Float { min, max, log } = *kind else { panic!("{id} is not continuous") };
        let x = ((v.clamp(min, max) - min) / (max - min)).clamp(0.0, 1.0);
        let n = if log {
            let t = ((min * max).sqrt() - min) / (max - min);
            x.powf(0.5f64.ln() / t.ln())
        } else {
            x
        };
        (id.to_owned(), Value::Index(i, n))
    }

    pub fn choice(&self, id: &str, v: &str) -> (String, Value) {
        let (i, kind) = self.find(id);
        let Kind::Choice(choices) = kind else { panic!("{id} is not a choice") };
        let k = choices.iter().position(|c| c.eq_ignore_ascii_case(v)).unwrap_or_else(|| panic!("{id} has no choice {v}"));
        (id.to_owned(), Value::Index(i, k as f64 / (choices.len().max(2) - 1) as f64))
    }

    pub fn flag(&self, id: &str, on: bool) -> (String, Value) {
        let (i, kind) = self.find(id);
        assert!(matches!(kind, Kind::Bool), "{id} is not a switch");
        (id.to_owned(), Value::Index(i, if on { 1.0 } else { 0.0 }))
    }
}
