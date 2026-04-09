//! Test common serialization interface that works with any Rust type
//!
//! RED PHASE: These tests define the desired interface and MUST FAIL initially

use qserde::{Deserialize, Serialize};

// ============================================================================
// Test 1: Primitive types should be serializable
// ============================================================================

#[test]
fn test_serialize_primitives() {
    // Test u8
    let value: u8 = 42;
    let bytes = value.serialize().expect("u8 should serialize");
    let restored: u8 = u8::deserialize(&bytes).expect("u8 should deserialize");
    assert_eq!(restored, value);

    // Test u64
    let value: u64 = u64::MAX;
    let bytes = value.serialize().expect("u64 should serialize");
    let restored: u64 = u64::deserialize(&bytes).expect("u64 should deserialize");
    assert_eq!(restored, value);

    // Test String
    let value = String::from("hello world");
    let bytes = value.serialize().expect("String should serialize");
    let restored: String = String::deserialize(&bytes).expect("String should deserialize");
    assert_eq!(restored, value);
}

// ============================================================================
// Test 2: Custom structs with derive should work
// ============================================================================

#[qserde::Archive]
#[derive(Debug, PartialEq)]
struct User {
    id: u64,
    name: String,
    email: String,
}

#[test]
fn test_serialize_custom_struct() {
    let user = User {
        id: 1,
        name: "Alice".to_string(),
        email: "alice@example.com".to_string(),
    };

    let bytes = user.serialize().expect("User should serialize");
    let restored: User = User::deserialize(&bytes).expect("User should deserialize");

    assert_eq!(restored, user);
}

// ============================================================================
// Test 3: Collections should be serializable
// ============================================================================

#[test]
fn test_serialize_vec() {
    let value = vec![1, 2, 3, 4, 5];
    let bytes = value.serialize().expect("Vec should serialize");
    let restored: Vec<i32> = Vec::deserialize(&bytes).expect("Vec should deserialize");
    assert_eq!(restored, value);
}

#[test]
fn test_serialize_hashmap() {
    use std::collections::HashMap;

    let mut map = HashMap::new();
    map.insert("key1".to_string(), 100);
    map.insert("key2".to_string(), 200);

    let bytes = map.serialize().expect("HashMap should serialize");
    let restored: HashMap<String, i32> =
        HashMap::deserialize(&bytes).expect("HashMap should deserialize");

    assert_eq!(restored.get("key1"), Some(&100));
    assert_eq!(restored.get("key2"), Some(&200));
}

// ============================================================================
// Test 4: Nested types should work
// ============================================================================

#[qserde::Archive]
#[derive(Debug, PartialEq)]
struct Config {
    name: String,
    values: Vec<String>,
    nested: NestedConfig,
}

#[qserde::Archive]
#[derive(Debug, PartialEq)]
struct NestedConfig {
    enabled: bool,
    count: u32,
}

#[test]
fn test_serialize_nested_types() {
    let config = Config {
        name: "test".to_string(),
        values: vec!["a".to_string(), "b".to_string()],
        nested: NestedConfig {
            enabled: true,
            count: 42,
        },
    };

    let bytes = config.serialize().expect("Config should serialize");
    let restored: Config = Config::deserialize(&bytes).expect("Config should deserialize");

    assert_eq!(restored, config);
}

// ============================================================================
// Test 5: Enums should be serializable
// ============================================================================

#[qserde::Archive]
#[derive(Debug, PartialEq)]
enum Status {
    Active,
    Inactive,
    Pending(String),
}

#[test]
fn test_serialize_enum() {
    let status = Status::Pending("waiting".to_string());

    let bytes = status.serialize().expect("Status should serialize");
    let restored: Status = Status::deserialize(&bytes).expect("Status should deserialize");

    assert_eq!(restored, status);
}

// ============================================================================
// Test 6: Option and Result types
// ============================================================================

#[test]
fn test_serialize_option() {
    let some_value: Option<i32> = Some(42);
    let none_value: Option<i32> = None;

    let bytes_some = some_value
        .serialize()
        .expect("Option::Some should serialize");
    let restored_some: Option<i32> =
        Option::deserialize(&bytes_some).expect("Option::Some should deserialize");
    assert_eq!(restored_some, some_value);

    let bytes_none = none_value
        .serialize()
        .expect("Option::None should serialize");
    let restored_none: Option<i32> =
        Option::deserialize(&bytes_none).expect("Option::None should deserialize");
    assert_eq!(restored_none, none_value);
}

// ============================================================================
// Test 7: Generic container - Archived<T> should work for any type
// ============================================================================

#[test]
fn test_archived_generic() {
    use qserde::Archived;

    let user = User {
        id: 99,
        name: "Bob".to_string(),
        email: "bob@example.com".to_string(),
    };

    let archived = Archived::from_value(&user).expect("Should create Archived");
    let restored: User = archived.load().expect("Should load from Archived");

    assert_eq!(restored, user);
}

// ============================================================================
// Test 8: Error handling for invalid data
// ============================================================================

#[test]
#[should_panic(expected = "buffer")]
fn test_deserialize_invalid_data() {
    let invalid_bytes = vec![0xFF, 0xFF, 0xFF, 0xFF];

    let _result: User = User::deserialize(&invalid_bytes).unwrap();
}

#[test]
#[should_panic(expected = "buffer")]
fn test_deserialize_empty_bytes() {
    let empty_bytes = vec![];

    let _result: User = User::deserialize(&empty_bytes).unwrap();
}

// ============================================================================
// Test 9: Large data structures
// ============================================================================

#[test]
fn test_large_vec() {
    let large_vec: Vec<u64> = (0..10_000).collect();

    let bytes = large_vec.serialize().expect("Large Vec should serialize");
    let restored: Vec<u64> = Vec::deserialize(&bytes).expect("Large Vec should deserialize");

    assert_eq!(restored.len(), 10_000);
    assert_eq!(restored, large_vec);
}

// ============================================================================
// Test 10: Zero-sized types
// ============================================================================

#[qserde::Archive]
#[derive(Debug, PartialEq)]
struct EmptyStruct;

#[test]
fn test_zero_sized_type() {
    let value = EmptyStruct;

    let bytes = value.serialize().expect("ZST should serialize");
    let restored: EmptyStruct = EmptyStruct::deserialize(&bytes).expect("ZST should deserialize");

    assert_eq!(restored, value);
}

// ============================================================================
// Test 11: Bytes extension trait for ergonomic API
// ============================================================================

#[test]
fn test_bytes_ext_trait() {
    use qserde::DeserializeExt;

    let user = User {
        id: 123,
        name: "Charlie".to_string(),
        email: "charlie@example.com".to_string(),
    };

    let bytes = user.serialize().expect("Should serialize");

    // Test that &[u8] has load method
    let restored: User = bytes
        .as_slice()
        .load()
        .expect("Should load from bytes slice");
    assert_eq!(restored, user);

    // Test that Vec<u8> has load method
    let restored2: User = bytes.load().expect("Should load from Vec<u8>");
    assert_eq!(restored2, user);
}
