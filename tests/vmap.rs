use std::collections::HashMap;
use runar_node::ValueType;

/// VMap wrapper for easier ValueType manipulation
#[derive(Debug, Clone)]
pub struct VMap(pub HashMap<String, ValueType>);

impl VMap {
    pub fn new() -> Self {
        VMap(HashMap::new())
    }

    pub fn from_hashmap(map: HashMap<String, ValueType>) -> Self {
        VMap(map)
    }

    pub fn from_value_type(value: ValueType) -> Self {
        match value {
            ValueType::Map(map) => VMap(map),
            _ => VMap::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&ValueType> {
        self.0.get(key)
    }

    pub fn get_string(&self, key: &str) -> Result<String, String> {
        match self.0.get(key) {
            Some(ValueType::String(s)) => Ok(s.clone()),
            Some(_) => Err(format!("Key '{}' exists but is not a string", key)),
            None => Err(format!("Key '{}' not found", key)),
        }
    }
}

/// Create a ValueType::Map from key-value pairs
#[macro_export]
macro_rules! vmap {
    ($($key:expr => $value:expr),* $(,)?) => {{
        let mut map = std::collections::HashMap::new();
        $(
            let key_str = $key.to_string();
            map.insert(key_str, $value.into());
        )*
        runar_node::ValueType::Map(map)
    }};
    () => {
        runar_node::ValueType::Map(std::collections::HashMap::new())
    };
}

/// Helper macro to extract values from a VMap by type, with proper error handling
#[macro_export]
macro_rules! vmap_extract {
    ($map:expr, $key:expr, String) => {
        $map.get_string($key)
    };
} 