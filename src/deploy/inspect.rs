use serde::Serialize;

/// A structured, render-agnostic snapshot of a unit's runtime state. Collection
/// (per-unit probes) is kept separate from formatting: the same tree feeds the
/// human renderer here and, later, a `--json` dump.
#[derive(Debug, Serialize)]
pub struct UnitReport {
    pub name: String,
    pub kind: &'static str,
    pub sections: Vec<Section>,
}

#[derive(Debug, Serialize)]
pub struct Section {
    pub title: &'static str,
    pub fields: Vec<Field>,
}

#[derive(Debug, Serialize)]
pub struct Field {
    pub label: String,
    pub value: String,
    pub health: Health,
}

/// Health signal carried per field. Not yet surfaced in the human render; kept
/// for the upcoming `--json` output and future status markers.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Ok,
    Warn,
    Down,
    Unknown,
}

impl UnitReport {
    pub fn new(name: impl Into<String>, kind: &'static str) -> Self {
        Self {
            name: name.into(),
            kind,
            sections: Vec::new(),
        }
    }

    pub fn push(&mut self, section: Section) {
        self.sections.push(section);
    }
}

impl Section {
    pub fn new(title: &'static str) -> Self {
        Self {
            title,
            fields: Vec::new(),
        }
    }

    pub fn push(&mut self, field: Field) {
        self.fields.push(field);
    }
}

impl Field {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            health: Health::Unknown,
        }
    }

    pub fn health(mut self, health: Health) -> Self {
        self.health = health;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_to_pretty_json() {
        let mut report = UnitReport::new("web", "app");
        let mut deploy = Section::new("deploy");
        deploy.push(Field::new("version", "3").health(Health::Ok));
        deploy.push(Field::new("status", "ready").health(Health::Ok));
        report.push(deploy);

        let json = serde_json::to_string_pretty(&report).unwrap();
        assert_eq!(
            json,
            r#"{
  "name": "web",
  "kind": "app",
  "sections": [
    {
      "title": "deploy",
      "fields": [
        {
          "label": "version",
          "value": "3",
          "health": "ok"
        },
        {
          "label": "status",
          "value": "ready",
          "health": "ok"
        }
      ]
    }
  ]
}"#,
        );
    }
}
