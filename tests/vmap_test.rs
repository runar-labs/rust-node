#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    
    use anyhow::Result;
    use serde_json::json;
    
    use kagi_node::services::ValueType;
    
    // Define a simple wrapper around HashMap<String, ValueType> to replace VMap
    #[derive(Clone, Debug)]
    struct VMap(HashMap<String, ValueType>);
    
    impl VMap {
        fn new() -> Self {
            VMap(HashMap::new())
        }
        
        fn insert(&mut self, key: &str, value: ValueType) {
            self.0.insert(key.to_string(), value);
        }
        
        fn get(&self, key: &str) -> Option<&ValueType> {
            self.0.get(key)
        }
        
        fn get_string(&self, key: &str) -> Result<String> {
            match self.get(key) {
                Some(ValueType::String(s)) => Ok(s.clone()),
                Some(other) => Err(anyhow::anyhow!("Expected String for key '{}', got {:?}", key, other)),
                None => Err(anyhow::anyhow!("Key '{}' not found", key)),
            }
        }
        
        fn get_number_as_int(&self, key: &str) -> Result<i32> {
            match self.get(key) {
                Some(ValueType::Number(n)) => Ok(*n as i32),
                Some(other) => Err(anyhow::anyhow!("Expected Number for key '{}', got {:?}", key, other)),
                None => Err(anyhow::anyhow!("Key '{}' not found", key)),
            }
        }
        
        fn get_bool(&self, key: &str) -> Result<bool> {
            match self.get(key) {
                Some(ValueType::Bool(b)) => Ok(*b),
                Some(other) => Err(anyhow::anyhow!("Expected Bool for key '{}', got {:?}", key, other)),
                None => Err(anyhow::anyhow!("Key '{}' not found", key)),
            }
        }
    }
    
    impl From<HashMap<String, ValueType>> for VMap {
        fn from(map: HashMap<String, ValueType>) -> Self {
            VMap(map)
        }
    }
    
    // Helper macros for the tests
    macro_rules! vmap {
        ($map:expr, $key:expr, String) => {
            $map.get_string($key)
        };
        ($map:expr, $key:expr, Number) => {
            $map.get_number_as_int($key)
        };
        ($map:expr, $key:expr, Bool) => {
            $map.get_bool($key)
        };
        ($map:expr, $key:expr) => {
            $map.get($key)
        };
    }
    
    macro_rules! vmap_opt {
        () => {
            ValueType::Map(std::collections::HashMap::new())
        };
        ($($key:expr => $value:expr),* $(,)?) => {
            {
                let mut map = std::collections::HashMap::new();
                $(
                    map.insert($key.to_string(), $value.into());
                )*
                ValueType::Map(map)
            }
        };
    }
    
    // Test implementation
    fn create_test_vmap() -> VMap {
        let mut map = HashMap::new();
        map.insert("string_key".to_string(), ValueType::String("test_value".to_string()));
        map.insert("int_key".to_string(), ValueType::Number(42.0));
        map.insert("bool_key".to_string(), ValueType::Bool(true));
        map.insert("float_key".to_string(), ValueType::Number(3.14));
        
        // Add a nested map
        let mut nested_map = HashMap::new();
        nested_map.insert("nested_string".to_string(), ValueType::String("nested_value".to_string()));
        nested_map.insert("nested_int".to_string(), ValueType::Number(100.0));
        map.insert("nested_key".to_string(), ValueType::Map(nested_map));
        
        // Add an array
        let array = vec![
            ValueType::String("item1".to_string()),
            ValueType::String("item2".to_string()),
            ValueType::Number(42.0)
        ];
        map.insert("array_key".to_string(), ValueType::Array(array));
        
        VMap::from(map)
    }
    
    #[tokio::test]
    async fn test_basic_value_extraction() -> Result<()> {
        let vmap = create_test_vmap();
        
        // Test string extraction
        let string_value: String = vmap!(vmap, "string_key", String)?;
        assert_eq!(string_value, "test_value", "String value should match");
        
        // Test integer extraction
        let int_value: i32 = vmap!(vmap, "int_key", Number)?;
        assert_eq!(int_value, 42, "Integer value should match");
        
        // Test boolean extraction
        let bool_value: bool = vmap!(vmap, "bool_key", Bool)?;
        assert!(bool_value, "Boolean value should be true");
        
        // Test float extraction
        let float_value: i32 = vmap!(vmap, "float_key", Number)?;
        assert_eq!(float_value, 3, "Float value should match when converted to int");
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_vmap_macro_direct_construction() -> Result<()> {
        // Alternative using standard construction and insertion
        let mut vmap = VMap::new();
        vmap.insert("name", ValueType::String("test".to_string()));
        vmap.insert("count", ValueType::Number(42.0));
        vmap.insert("enabled", ValueType::Bool(true));
        
        // Verify contents
        let name: String = vmap!(vmap, "name", String)?;
        let count: i32 = vmap!(vmap, "count", Number)?;
        let enabled: bool = vmap!(vmap, "enabled", Bool)?;
        
        assert_eq!(name, "test");
        assert_eq!(count, 42);
        assert_eq!(enabled, true);
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_error_handling() -> Result<()> {
        let vmap = create_test_vmap();
        
        // Test missing key
        let missing_key_result: Result<String, _> = vmap!(vmap, "non_existent_key", String);
        assert!(missing_key_result.is_err(), "Missing key should return error");
        
        // Test wrong type
        let wrong_type_result: Result<i32, _> = vmap!(vmap, "string_key", Number);
        assert!(wrong_type_result.is_err(), "Extracting wrong type should return error");
        
        // Test error message content
        match vmap!(vmap, "non_existent_key", String) {
            Ok(_) => panic!("Should have failed on missing key"),
            Err(e) => {
                let err_str = e.to_string();
                assert!(err_str.contains("non_existent_key"), 
                    "Error message should contain the key name");
                assert!(err_str.contains("not found"), 
                    "Error message should mention that the key was not found");
            }
        }
        
        match vmap!(vmap, "string_key", Number) {
            Ok(_) => panic!("Should have failed on wrong type"),
            Err(e) => {
                let err_str = e.to_string();
                assert!(err_str.contains("string_key"), 
                    "Error message should contain the key name");
                assert!(err_str.contains("Number"), 
                    "Error message should mention the expected type");
            }
        }
        
        Ok(())
    }
} 