use crate::versions::{Clock, Relation};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "hash", rename_all = "kebab-case")]
pub enum Content {
    File(String),
    Directory,
    Deleted,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    pub clock: Clock,
    pub content: Content,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Versions {
    pub heads: Vec<Revision>,
}

impl Revision {
    pub fn id(&self) -> Result<String> {
        Ok(blake3::hash(&serde_json::to_vec(self)?)
            .to_hex()
            .to_string())
    }
}

impl Versions {
    pub fn edit(&self, device: &str, content: Content) -> Result<Self> {
        let clock = self
            .heads
            .iter()
            .fold(Clock::default(), |v, h| v.merge(&h.clock));
        Ok(Self {
            heads: vec![Revision {
                clock: clock.advance(device)?,
                content,
            }],
        })
    }

    pub fn join(&self, other: &Self) -> Result<Self> {
        ensure!(self.heads.len() <= 128 && other.heads.len() <= 128, "too many concurrent versions");
        let mut candidates = Vec::new();
        for revision in self.heads.iter().chain(&other.heads) {
            revision.clock.validate()?;
            if let Content::File(hash) = &revision.content {
                ensure!(hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()), "invalid content hash");
            }
            candidates.push((revision.id()?, revision.clone()));
        }
        candidates.sort_by(|a, b| a.0.cmp(&b.0));
        candidates.dedup_by(|a, b| a.0 == b.0);
        for (_, a) in &candidates {
            for (_, b) in &candidates {
                ensure!(a.clock.relation(&b.clock) != Relation::Equal || a.content == b.content, "equal clock with different content");
            }
        }
        let heads: Vec<_> = candidates.iter().filter(|(_, candidate)| {
            !candidates.iter().any(|(_, other)| candidate.clock.relation(&other.clock) == Relation::Before)
        }).map(|(_, revision)| revision.clone()).collect();
        ensure!(heads.len() <= 128, "too many concurrent versions");
        Ok(Self { heads })
    }

    pub fn selected(&self) -> Option<&Revision> {
        self.heads.iter().min_by_key(|h| {
            let rank = match h.content {
                Content::File(_) => 0,
                Content::Directory => 1,
                Content::Deleted => 2,
            };
            (rank, h.id().unwrap_or_default())
        })
    }
}

pub fn validate_path(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && path.len() <= 4096,
        "invalid path length"
    );
    for name in path.split('/') {
        ensure!(
            !name.is_empty() && name != "." && name != ".." && name.len() <= 240,
            "unsafe path component"
        );
        ensure!(
            !name.to_ascii_lowercase().starts_with(".everywhere-"),
            "reserved application path"
        );
        ensure!(
            !name.ends_with(['.', ' ']),
            "nonportable trailing dot or space"
        );
        ensure!(
            !name
                .chars()
                .any(|c| c.is_control() || "<>:\"\\|?*".contains(c)),
            "nonportable filename"
        );
        let stem = name.split('.').next().unwrap().to_ascii_uppercase();
        ensure!(
            !["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str()),
            "reserved Windows filename"
        );
        ensure!(
            !((stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.len() == 4
                && matches!(stem.as_bytes()[3], b'1'..=b'9')),
            "reserved Windows filename"
        );
    }
    Ok(())
}

/// A conservative cross-platform alias key; never silently rename an alias.
pub fn collision_key(path: &str) -> String {
    path.nfc().flat_map(char::to_uppercase).collect()
}
