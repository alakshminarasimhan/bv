use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// A record of one conda package's OCI layer blob on the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    /// Content-addressed digest of the compressed layer blob ("sha256:...").
    pub digest: String,
    /// How many distinct tool builds have included this package as a solo layer.
    /// Higher count = higher priority for the solo slot when max_layers is tight.
    pub count: u64,
}

/// Maps conda package builds to their reproducible OCI layer digest.
///
/// Key format: "name==version-build" (e.g. "openssl==3.3.0-h69704a7_0").
///
/// Because bv-builder produces bit-identical compressed layer blobs for the
/// same package triple (SOURCE_DATE_EPOCH + sorted entries + zstd level 19),
/// the digest here is not a cache hint — it is a stable identity. Any future
/// build of the same package will produce the same bytes and therefore the
/// same blob on the registry. Docker/OCI registries deduplicate by content
/// digest, so two images that both contain "openssl==3.3.0" share exactly one
/// copy of that layer on disk and on the wire.
///
/// The catalog is stored in the registry repo at `layers/catalog.json` and
/// grows incrementally as new tools are published via `bv publish --spec`.
/// It replaces the batch-computed `popularity.json` for user-side publishing:
/// instead of requiring a full scan of all registry specs, each new tool
/// greedily adds its solo layers to the catalog, and future builds inherit
/// the benefit automatically.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LayerCatalog {
    pub version: u32,
    pub entries: HashMap<String, CatalogEntry>,
}

impl LayerCatalog {
    pub fn new() -> Self {
        Self {
            version: 1,
            entries: HashMap::new(),
        }
    }

    pub fn key(name: &str, version: &str, build: &str) -> String {
        format!("{name}=={version}-{build}")
    }

    pub fn get(&self, name: &str, version: &str, build: &str) -> Option<&CatalogEntry> {
        self.entries.get(&Self::key(name, version, build))
    }

    pub fn contains(&self, name: &str, version: &str, build: &str) -> bool {
        self.entries.contains_key(&Self::key(name, version, build))
    }

    /// Record a solo layer for this package. If the entry already exists the
    /// count is incremented and the digest is updated (same package triple
    /// always produces the same digest, so the update is a no-op in practice).
    pub fn record(&mut self, name: &str, version: &str, build: &str, digest: &str) {
        let key = Self::key(name, version, build);
        let entry = self.entries.entry(key).or_insert(CatalogEntry {
            digest: digest.to_string(),
            count: 0,
        });
        entry.digest = digest.to_string();
        entry.count += 1;
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let s = std::fs::read_to_string(path)
            .with_context(|| format!("read layer catalog '{}'", path.display()))?;
        serde_json::from_str(&s)
            .with_context(|| format!("parse layer catalog '{}'", path.display()))
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, &json)
            .with_context(|| format!("write layer catalog '{}'", path.display()))
    }

    pub fn to_json(&self) -> anyhow::Result<String> {
        serde_json::to_string_pretty(self).context("serialize catalog")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_format() {
        assert_eq!(
            LayerCatalog::key("openssl", "3.3.0", "h69704a7_0"),
            "openssl==3.3.0-h69704a7_0"
        );
    }

    #[test]
    fn record_increments_count() {
        let mut cat = LayerCatalog::new();
        cat.record("openssl", "3.3.0", "h0", "sha256:abc");
        cat.record("openssl", "3.3.0", "h0", "sha256:abc");
        let entry = cat.get("openssl", "3.3.0", "h0").unwrap();
        assert_eq!(entry.count, 2);
        assert_eq!(entry.digest, "sha256:abc");
    }

    #[test]
    fn round_trips_json() {
        let mut cat = LayerCatalog::new();
        cat.record("zlib", "1.2.11", "h0_0", "sha256:xyz");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        cat.save(&path).unwrap();
        let loaded = LayerCatalog::load(&path).unwrap();
        assert!(loaded.contains("zlib", "1.2.11", "h0_0"));
    }
}
