// vmap_macros.rs
//
// This file provides macros for working with ValueType maps and extracting values
// in a type-safe and ergonomic way.

use std::collections::HashMap;
// Import ValueType from runar_common instead of the local module
use runar_common::types::ValueType;

// NOTE: Macros are defined in services/mod.rs and exported at the crate root
// These definitions are commented out to prevent redefinition errors

/*
#[macro_export]
macro_rules! vmap {
    // Empty map
    {} => {
        {
            let map: std::collections::HashMap<String, $crate::services::ValueType> = std::collections::HashMap::new();
            $crate::services::ValueType::Map(map)
        }
    };

    // Map with entries
    {
        $($key:expr => $value:expr),* $(,)?
    } => {
        {
            let mut map = std::collections::HashMap::new();
            $(
                map.insert($key.to_string(), $crate::services::ValueType::from($value));
            )*
            $crate::services::ValueType::Map(map)
        }
    };
}

#[macro_export]
macro_rules! vmap_opt {
    // Empty map
    {} => {
        {
            let mut map = std::collections::HashMap::new();
            Some($crate::services::ValueType::Map(map))
        }
    };

    // Map with entries
    {
        $($key:expr => $value:expr),* $(,)?
    } => {
        {
            let mut map = std::collections::HashMap::new();
            $(
                map.insert($key.to_string(), $crate::services::ValueType::from($value));
            )*
            Some($crate::services::ValueType::Map(map))
        }
    };
}

#[macro_export]
macro_rules! vmap_extract {
    ($map:expr, $key:expr, $default:expr) => {
        {
            $crate::vmap_macros::extract_value(&$map, $key, $default)
        }
    };
}

#[macro_export]
macro_rules! vmap_direct {
    ($value:expr, $default:expr) => {
        {
            $crate::vmap_macros::extract_direct(&$value, $default)
        }
    };
}
*/

/// Helper function to convert a ValueType to a specified type
/// 
/// This function attempts to deserialize a ValueType into the target type.
/// If deserialization fails, it returns the provided default value.
/// 
/// # Examples
/// 
/// ```
/// let value = ValueType::Number(30.0);
/// let age: i32 = value_to_type(&value, 0);
/// assert_eq!(age, 30);
/// ```
pub fn value_to_type<T: serde::de::DeserializeOwned>(value: &ValueType, default: T) -> T {
    match serde_json::to_value(value) {
        Ok(json_value) => {
            match serde_json::from_value::<T>(json_value) {
                Ok(typed_value) => typed_value,
                Err(_) => default,
            }
        },
        Err(_) => default,
    }
}

/// Extract a value from a ValueType::Map by key
pub fn extract_value<T: serde::de::DeserializeOwned>(
    map: &ValueType, 
    key: &str, 
    default: T
) -> T {
    if let ValueType::Map(map_data) = map {
        if let Some(value) = map_data.get(key) {
            return value_to_type(value, default);
        }
    }
    default
}

/// Extract a value directly from a ValueType
pub fn extract_direct<T: serde::de::DeserializeOwned>(value: &ValueType, default: T) -> T {
    value_to_type(value, default)
} 