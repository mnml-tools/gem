//! Synthetic Rust library used by the Gem test servers.

use std::collections::HashMap;

/// A tiny in-memory key-value store.
#[derive(Debug, Default)]
pub struct Store {
    items: HashMap<String, String>,
}

impl Store {
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) -> Option<String> {
        self.items.insert(key.into(), value.into())
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.items.get(key).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let mut store = Store::default();
        store.insert("a", "1");
        assert_eq!(store.get("a"), Some("1"));
    }
}
