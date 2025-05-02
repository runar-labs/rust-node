use std::collections::HashMap;
use anyhow::Result;
use serde::{Serialize, Deserialize};
use runar_common::types::ValueType;
use runar_node::network::transport::{NetworkMessage, NetworkMessagePayloadItem, PeerId};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct TestStruct {
    id: String,
    value: i32,
    metadata: HashMap<String, String>,
}

#[test]
fn test_message_payload_item_serialization() -> Result<()> {
    // Create a simple payload item
    let payload_item = NetworkMessagePayloadItem::new(
        "test/path".to_string(),
        ValueType::String("test-value".to_string()),
        "correlation-123".to_string()
    );
    
    // Serialize the payload item
    let serialized = bincode::serialize(&payload_item)?;
    println!("Serialized payload item to {} bytes", serialized.len());
    
    // Deserialize the payload item
    let deserialized: NetworkMessagePayloadItem = bincode::deserialize(&serialized)?;
    
    // Verify the data matches
    assert_eq!(deserialized.path, "test/path");
    assert_eq!(deserialized.correlation_id, "correlation-123");
    if let ValueType::String(s) = &deserialized.value {
        assert_eq!(s, "test-value");
    } else {
        panic!("Expected ValueType::String");
    }
    
    println!("Test passed: Payload item serialization/deserialization works");
    Ok(())
}

#[test]
fn test_network_message_serialization() -> Result<()> {
    // Create source and destination IDs
    let source_id = PeerId::new("source-node".to_string());
    let dest_id = PeerId::new("dest-node".to_string());
    
    // Create a payload
    let payload_item = NetworkMessagePayloadItem::new(
        "test/path".to_string(),
        ValueType::Number(42.0),
        "correlation-456".to_string()
    );
    
    // Create a network message
    let message = NetworkMessage {
        source: source_id.clone(),
        destination: dest_id.clone(),
        message_type: "TestMessage".to_string(),
        payloads: vec![payload_item],
    };
    
    // Serialize the message
    let serialized = bincode::serialize(&message)?;
    println!("Serialized network message to {} bytes", serialized.len());
    
    // Deserialize the message
    let deserialized: NetworkMessage = bincode::deserialize(&serialized)?;
    
    // Verify the data matches
    assert_eq!(deserialized.source.public_key, source_id.public_key);
    assert_eq!(deserialized.destination.public_key, dest_id.public_key);
    assert_eq!(deserialized.message_type, "TestMessage");
    assert_eq!(deserialized.payloads.len(), 1);
    assert_eq!(deserialized.payloads[0].path, "test/path");
    assert_eq!(deserialized.payloads[0].correlation_id, "correlation-456");
    if let ValueType::Number(n) = deserialized.payloads[0].value {
        assert_eq!(n, 42.0);
    } else {
        panic!("Expected ValueType::Number");
    }
    
    println!("Test passed: Network message serialization/deserialization works");
    Ok(())
}

// Now try with the struct-based approach
#[test]
fn test_struct_serialization_in_network_message() -> Result<()> {
    // Create test data
    let mut metadata = HashMap::new();
    metadata.insert("type".to_string(), "test".to_string());
    metadata.insert("version".to_string(), "1.0".to_string());
    
    let original = TestStruct {
        id: "test-struct-123".to_string(),
        value: 42,
        metadata: metadata.clone(),
    };
    
    // Create source and destination IDs
    let source_id = PeerId::new("source-node".to_string());
    let dest_id = PeerId::new("dest-node".to_string());
    
    // Create a payload with the struct - directly using bincode
    let payload_item = NetworkMessagePayloadItem::with_struct(
        "test/path".to_string(),
        original.clone(),
        "test-correlation-123".to_string()
    )?;
    
    // Verify we can use both extract_struct and deserialize_to
    let extracted1: TestStruct = payload_item.extract_struct()?;
    let extracted2: TestStruct = payload_item.value.deserialize_to()?;
    
    assert_eq!(extracted1, original, "extract_struct failed");
    assert_eq!(extracted2, original, "deserialize_to failed");
    
    // Create a network message
    let message = NetworkMessage {
        source: source_id,
        destination: dest_id,
        message_type: "TestMessage".to_string(),
        payloads: vec![payload_item],
    };
    
    // Serialize the entire message using bincode
    let serialized_message = bincode::serialize(&message)?;
    println!("Serialized struct-based network message to {} bytes", serialized_message.len());
    
    // Deserialize the message
    let deserialized_message: NetworkMessage = bincode::deserialize(&serialized_message)?;
    
    // Both methods should work for extracting the struct
    let extracted_struct1: TestStruct = deserialized_message.payloads[0].extract_struct()?;
    let extracted_struct2: TestStruct = deserialized_message.payloads[0].value.deserialize_to()?;
    
    // Verify the data
    assert_eq!(extracted_struct1, original, "Extracted struct using extract_struct doesn't match original");
    assert_eq!(extracted_struct2, original, "Extracted struct using deserialize_to doesn't match original");
    assert_eq!(deserialized_message.message_type, "TestMessage");
    assert_eq!(deserialized_message.payloads[0].path, "test/path");
    assert_eq!(deserialized_message.payloads[0].correlation_id, "test-correlation-123");
    
    println!("Test passed: Direct bincode serialization and deserialization works with both methods");
    Ok(())
}

#[test]
fn test_multiple_struct_types_in_message() -> Result<()> {
    // Create test data with two different struct types
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct UserData {
        username: String,
        email: String,
        age: u32,
    }
    
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct ProductData {
        name: String,
        price: f64,
        stock: u32,
    }
    
    let user = UserData {
        username: "testuser".to_string(),
        email: "test@example.com".to_string(),
        age: 30,
    };
    
    let product = ProductData {
        name: "Test Product".to_string(),
        price: 99.99,
        stock: 10,
    };
    
    // Create source and destination IDs
    let source_id = PeerId::new("source-node".to_string());
    let dest_id = PeerId::new("dest-node".to_string());
    
    // Create payloads with different struct types
    let user_payload = NetworkMessagePayloadItem::with_struct(
        "users/data".to_string(),
        user.clone(),
        "user-123".to_string()
    )?;
    
    let product_payload = NetworkMessagePayloadItem::with_struct(
        "products/data".to_string(),
        product.clone(),
        "product-456".to_string()
    )?;
    
    // Create a network message with multiple payloads
    let message = NetworkMessage {
        source: source_id,
        destination: dest_id,
        message_type: "MultiStructMessage".to_string(),
        payloads: vec![user_payload, product_payload],
    };
    
    // Serialize the entire message
    let serialized_message = bincode::serialize(&message)?;
    println!("Serialized multi-struct message to {} bytes", serialized_message.len());
    
    // Deserialize the message
    let deserialized_message: NetworkMessage = bincode::deserialize(&serialized_message)?;
    
    // Extract the structs from the payloads
    let extracted_user: UserData = deserialized_message.payloads[0].extract_struct()?;
    let extracted_product: ProductData = deserialized_message.payloads[1].extract_struct()?;
    
    // Verify the data
    assert_eq!(extracted_user, user, "Extracted user doesn't match original");
    assert_eq!(extracted_product, product, "Extracted product doesn't match original");
    assert_eq!(deserialized_message.payloads[0].path, "users/data");
    assert_eq!(deserialized_message.payloads[1].path, "products/data");
    
    println!("Multi-struct test passed: All data correctly serialized and deserialized");
    Ok(())
}

#[test]
fn test_various_types_in_network_messages() -> Result<()> {
    // Create source and destination IDs
    let source_id = PeerId::new("source-node".to_string());
    let dest_id = PeerId::new("dest-node".to_string());
    
    // Create a struct
    let mut metadata = HashMap::new();
    metadata.insert("type".to_string(), "test".to_string());
    
    let test_struct = TestStruct {
        id: "struct-test".to_string(),
        value: 42,
        metadata: metadata.clone(),
    };
    
    // Create a map
    let mut test_map = HashMap::new();
    test_map.insert("key1".to_string(), "value1".to_string());
    test_map.insert("key2".to_string(), "value2".to_string());
    
    // Create an array
    let test_array = vec![1, 2, 3, 4, 5];
    
    // Create payload items for each type
    let struct_payload = NetworkMessagePayloadItem::with_struct(
        "test/struct".to_string(),
        test_struct.clone(),
        "correlation-struct".to_string()
    )?;
    
    let map_payload = NetworkMessagePayloadItem::with_map(
        "test/map".to_string(),
        test_map.clone(),
        "correlation-map".to_string()
    )?;
    
    let array_payload = NetworkMessagePayloadItem::with_array(
        "test/array".to_string(),
        test_array.clone(),
        "correlation-array".to_string()
    )?;
    
    // Create a network message with all payloads
    let message = NetworkMessage {
        source: source_id,
        destination: dest_id,
        message_type: "TestAllTypes".to_string(),
        payloads: vec![struct_payload, map_payload, array_payload],
    };
    
    // Serialize the message
    let serialized = bincode::serialize(&message)?;
    println!("Serialized network message with all types to {} bytes", serialized.len());
    
    // Deserialize the message
    let deserialized: NetworkMessage = bincode::deserialize(&serialized)?;
    
    // Verify struct payload
    let extracted_struct: TestStruct = deserialized.payloads[0].extract()?;
    assert_eq!(extracted_struct, test_struct, "Extracted struct doesn't match original");
    println!("✓ Struct extraction works");
    
    // Verify map payload
    let extracted_map: HashMap<String, String> = deserialized.payloads[1].extract_map()?;
    assert_eq!(extracted_map, test_map, "Extracted map doesn't match original");
    println!("✓ Map extraction works");
    
    // Verify array payload
    let extracted_array: Vec<i32> = deserialized.payloads[2].extract_array()?;
    assert_eq!(extracted_array, test_array, "Extracted array doesn't match original");
    println!("✓ Array extraction works");
    
    // Try using the generic extract method for all types
    let extracted_struct2: TestStruct = deserialized.payloads[0].extract()?;
    let extracted_map2: HashMap<String, String> = deserialized.payloads[1].extract()?;
    let extracted_array2: Vec<i32> = deserialized.payloads[2].extract()?;
    
    assert_eq!(extracted_struct2, test_struct, "Generic extract of struct failed");
    assert_eq!(extracted_map2, test_map, "Generic extract of map failed");
    assert_eq!(extracted_array2, test_array, "Generic extract of array failed");
    
    println!("✓ Generic extract method works for all types");
    println!("All tests passed for various data types in network messages");
    
    Ok(())
} 