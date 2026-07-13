/// A generic cache.
pub struct Cache<K, V> {
    entries: Vec<(K, V)>,
}

/// Loads a value by key.
pub async fn load<K: AsRef<str>, V: Default>(key: K) -> V {
    let _ = key;
    V::default()
}
