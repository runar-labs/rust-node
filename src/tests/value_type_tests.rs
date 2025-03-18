#[cfg(test)]
mod tests {
    use crate::services::ValueType;
    use crate::{vjson, vmap};
    use std::collections::HashMap;

    #[test]
    fn test_vjson_macro() {
        // Test creating a ValueType using the vjson! macro
        let value = vjson!({
            "name": "John",
            "age": 30,
            "is_active": true,
            "address": {
                "street": "123 Main St",
                "city": "Anytown"
            },
            "skills": ["Rust", "JavaScript", "Python"]
        });

        // Convert to JSON value for easier testing
        let json = value.to_json();
        assert_eq!(json["name"], "John");
        assert_eq!(json["age"], 30);
        assert_eq!(json["is_active"], true);
        assert_eq!(json["address"]["street"], "123 Main St");
        assert_eq!(json["address"]["city"], "Anytown");
        assert_eq!(json["skills"][0], "Rust");
        assert_eq!(json["skills"][1], "JavaScript");
        assert_eq!(json["skills"][2], "Python");
    }

    #[test]
    fn test_vmap_macro() {
        // Test creating a ValueType using the vmap! macro
        let value = vmap! {
            "name" => "John",
            "age" => 30,
            "is_active" => true,
            "address" => vmap! {
                "street" => "123 Main St",
                "city" => "Anytown"
            }
        };

        // Now use the ValueType API to interact with it
        let map = match &value {
            ValueType::Map(m) => m,
            _ => panic!("Expected a Map ValueType"),
        };

        // Check top-level fields
        match &map["name"] {
            ValueType::String(s) => assert_eq!(s, "John"),
            _ => panic!("Expected a String ValueType for name"),
        }

        match &map["age"] {
            ValueType::Number(n) => assert_eq!(*n, 30.0),
            _ => panic!("Expected a Number ValueType for age"),
        }

        match &map["is_active"] {
            ValueType::Bool(b) => assert_eq!(*b, true),
            _ => panic!("Expected a Bool ValueType for is_active"),
        }

        // Check nested map
        match &map["address"] {
            ValueType::Map(addr) => {
                // Debug print to see what's actually in the address map
                println!("Address map: {:?}", addr);

                // Debug print to see what type the street value is
                match &addr["street"] {
                    ValueType::String(_) => println!("street is ValueType::String"),
                    ValueType::Json(_) => println!("street is ValueType::Json"),
                    ValueType::Number(_) => println!("street is ValueType::Number"),
                    ValueType::Bool(_) => println!("street is ValueType::Bool"),
                    ValueType::Array(_) => println!("street is ValueType::Array"),
                    ValueType::Map(_) => println!("street is ValueType::Map"),
                    ValueType::Bytes(_) => println!("street is ValueType::Bytes"),
                    ValueType::Null => println!("street is ValueType::Null"),
                    ValueType::Struct(_) => println!("street is ValueType::Struct"),
                }

                match &addr["street"] {
                    ValueType::String(s) => assert_eq!(s, "123 Main St"),
                    _ => panic!("Expected a String ValueType for street"),
                }
                match &addr["city"] {
                    ValueType::String(s) => assert_eq!(s, "Anytown"),
                    _ => panic!("Expected a String ValueType for city"),
                }
            }
            _ => panic!("Expected a Map ValueType for address"),
        }
    }

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    struct User {
        name: String,
        age: u32,
        is_active: bool,
    }

    #[test]
    fn test_struct_to_value_type() {
        // Create a struct
        let user = User {
            name: "John".to_string(),
            age: 30,
            is_active: true,
        };

        // Convert to a ValueType
        let value = crate::services::to_value_type(user);

        // Get as JSON and verify
        let json = value.to_json();
        assert_eq!(json["name"], "John");
        assert_eq!(json["age"], 30.0);
        assert_eq!(json["is_active"], true);
    }
}
