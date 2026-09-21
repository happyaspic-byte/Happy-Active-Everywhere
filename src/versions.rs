use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Clock(BTreeMap<String, u64>);
#[derive(Debug, PartialEq, Eq)]
pub enum Relation {
    Before,
    After,
    Equal,
    Concurrent,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    SendOnly,
    ReceiveOnly,
    Bidirectional,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Apply,
    Ignore,
    Conflict,
    Merge,
    Invalid,
}
impl Clock {
    pub fn advance(&self, device: &str) -> Result<Self> {
        ensure!(
            !device.is_empty() && device.len() <= 128,
            "invalid device identifier"
        );
        let mut next = self.clone();
        let value = next.0.entry(device.to_owned()).or_default();
        *value = value.checked_add(1).context("device counter exhausted")?;
        Ok(next)
    }
    pub fn merge(&self, other: &Self) -> Self {
        let mut result = self.clone();
        for (device, count) in &other.0 {
            let value = result.0.entry(device.clone()).or_default();
            *value = (*value).max(*count);
        }
        result
    }
    pub fn relation(&self, other: &Self) -> Relation {
        let mut less = false;
        let mut greater = false;
        for device in self.0.keys().chain(other.0.keys()) {
            let a = self.0.get(device).copied().unwrap_or(0);
            let b = other.0.get(device).copied().unwrap_or(0);
            less |= a < b;
            greater |= a > b;
        }
        match (less, greater) {
            (true, true) => Relation::Concurrent,
            (true, false) => Relation::Before,
            (false, true) => Relation::After,
            (false, false) => Relation::Equal,
        }
    }
}
pub fn reconcile(
    mode: Mode,
    local: Option<(&Clock, Option<&str>)>,
    remote: (&Clock, Option<&str>),
) -> Decision {
    if mode == Mode::SendOnly {
        return Decision::Ignore;
    }
    let Some((clock, content)) = local else {
        return Decision::Apply;
    };
    match clock.relation(remote.0) {
        Relation::Equal if content != remote.1 => Decision::Invalid,
        Relation::Equal | Relation::After => Decision::Ignore,
        Relation::Before => Decision::Apply,
        Relation::Concurrent if content == remote.1 => Decision::Merge,
        Relation::Concurrent => Decision::Conflict,
    }
}
