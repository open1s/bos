//! Tests for serialization backends

#[cfg(feature = "rkyv-backend")]
mod rkyv_tests {
    use qserde::backends::RkyvBackend;

    #[derive(
        Debug,
        PartialEq,
        serde::Serialize,
        serde::Deserialize,
        rkyv::Archive,
        rkyv::Serialize,
        rkyv::Deserialize,
    )]
    struct TestStruct {
        id: u64,
        name: String,
        value: f64,
    }

    #[test]
    fn test_rkyv_serialize_primitive() {
        let backend = RkyvBackend;
        let value: u32 = 42;

        let bytes = backend.serialize(&value).expect("serialize u32");
        let restored: u32 = backend.deserialize(&bytes).expect("deserialize u32");

        assert_eq!(value, restored);
    }

    #[test]
    fn test_rkyv_serialize_string() {
        let backend = RkyvBackend;
        let value = "hello world".to_string();

        let bytes = backend.serialize(&value).expect("serialize String");
        let restored: String = backend.deserialize(&bytes).expect("deserialize String");

        assert_eq!(value, restored);
    }

    #[test]
    fn test_rkyv_serialize_struct() {
        let backend = RkyvBackend;
        let value = TestStruct {
            id: 123,
            name: "test".to_string(),
            value: 3.14,
        };

        let bytes = backend.serialize(&value).expect("serialize struct");
        let restored: TestStruct = backend.deserialize(&bytes).expect("deserialize struct");

        assert_eq!(value.id, restored.id);
        assert_eq!(value.name, restored.name);
        assert!((value.value - restored.value).abs() < 0.001);
    }

    #[test]
    fn test_rkyv_serialize_vec() {
        let backend = RkyvBackend;
        let value = vec![1u32, 2, 3, 4, 5];

        let bytes = backend.serialize(&value).expect("serialize vec");
        let restored: Vec<u32> = backend.deserialize(&bytes).expect("deserialize vec");

        assert_eq!(value, restored);
    }

    #[test]
    fn test_rkyv_serialize_option() {
        let backend = RkyvBackend;

        // Some value
        let value: Option<i32> = Some(42);
        let bytes = backend.serialize(&value).expect("serialize Option");
        let restored: Option<i32> = backend.deserialize(&bytes).expect("deserialize Option");
        assert_eq!(value, restored);

        // None value
        let value: Option<i32> = None;
        let bytes = backend.serialize(&value).expect("serialize None");
        let restored: Option<i32> = backend.deserialize(&bytes).expect("deserialize None");
        assert_eq!(value, restored);
    }

    #[test]
    fn test_rkyv_serialize_tuple() {
        let backend = RkyvBackend;
        let value = (1u32, "hello".to_string(), true);

        let bytes = backend.serialize(&value).expect("serialize tuple");
        let restored: (u32, String, bool) = backend.deserialize(&bytes).expect("deserialize tuple");

        assert_eq!(value, restored);
    }
}

#[cfg(feature = "serde-backend")]
mod serde_json_tests {
    use qserde::backends::SerdeJsonBackend;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct TestStruct {
        id: u64,
        name: String,
        tags: Vec<String>,
    }

    #[test]
    fn test_json_serialize_primitive() {
        let backend = SerdeJsonBackend;
        let value: u32 = 42;

        let bytes = backend.serialize(&value).expect("serialize u32");
        let restored: u32 = backend.deserialize(&bytes).expect("deserialize u32");

        assert_eq!(value, restored);
    }

    #[test]
    fn test_json_serialize_string() {
        let backend = SerdeJsonBackend;
        let value = "hello world".to_string();

        let bytes = backend.serialize(&value).expect("serialize String");
        let restored: String = backend.deserialize(&bytes).expect("deserialize String");

        assert_eq!(value, restored);
    }

    #[test]
    fn test_json_serialize_struct() {
        let backend = SerdeJsonBackend;
        let value = TestStruct {
            id: 123,
            name: "test".to_string(),
            tags: vec!["a".to_string(), "b".to_string()],
        };

        let bytes = backend.serialize(&value).expect("serialize struct");
        let restored: TestStruct = backend.deserialize(&bytes).expect("deserialize struct");

        assert_eq!(value, restored);
    }

    #[test]
    fn test_json_serialize_bool() {
        let backend = SerdeJsonBackend;

        let bytes = backend.serialize(&true).expect("serialize bool");
        let restored: bool = backend.deserialize(&bytes).expect("deserialize bool");
        assert!(restored);

        let bytes = backend.serialize(&false).expect("serialize false");
        let restored: bool = backend.deserialize(&bytes).expect("deserialize false");
        assert!(!restored);
    }

    #[test]
    fn test_json_roundtrip_mixed() {
        let backend = SerdeJsonBackend;
        let value = vec![
            TestStruct {
                id: 1,
                name: "one".to_string(),
                tags: vec![],
            },
            TestStruct {
                id: 2,
                name: "two".to_string(),
                tags: vec!["x".to_string()],
            },
        ];

        let bytes = backend.serialize(&value).expect("serialize vec");
        let restored: Vec<TestStruct> = backend.deserialize(&bytes).expect("deserialize vec");

        assert_eq!(value, restored);
    }
}

#[cfg(feature = "cbor-backend")]
mod cbor_tests {
    use qserde::backends::CborBackend;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct TestStruct {
        id: u64,
        name: String,
        data: Vec<u8>,
    }

    #[test]
    fn test_cbor_serialize_primitive() {
        let backend = CborBackend;
        let value: i64 = -12345;

        let bytes = backend.serialize(&value).expect("serialize i64");
        let restored: i64 = backend.deserialize(&bytes).expect("deserialize i64");

        assert_eq!(value, restored);
    }

    #[test]
    fn test_cbor_serialize_string() {
        let backend = CborBackend;
        let value = "Unicode: 你好世界 🌍".to_string();

        let bytes = backend.serialize(&value).expect("serialize String");
        let restored: String = backend.deserialize(&bytes).expect("deserialize String");

        assert_eq!(value, restored);
    }

    #[test]
    fn test_cbor_serialize_bytes() {
        let backend = CborBackend;
        let value = vec![0u8, 1, 255, 128, 64];

        let bytes = backend.serialize(&value).expect("serialize bytes");
        let restored: Vec<u8> = backend.deserialize(&bytes).expect("deserialize bytes");

        assert_eq!(value, restored);
    }

    #[test]
    fn test_cbor_serialize_struct() {
        let backend = CborBackend;
        let value = TestStruct {
            id: 999,
            name: "binary test".to_string(),
            data: vec![1, 2, 3, 4],
        };

        let bytes = backend.serialize(&value).expect("serialize struct");
        let restored: TestStruct = backend.deserialize(&bytes).expect("deserialize struct");

        assert_eq!(value, restored);
    }
}

#[cfg(feature = "bincode-backend")]
mod bincode_tests {
    use qserde::backends::BincodeBackend;

    #[derive(
        Debug, PartialEq, serde::Serialize, serde::Deserialize, bincode::Encode, bincode::Decode,
    )]
    struct TestStruct {
        id: u64,
        name: String,
        count: usize,
    }

    #[test]
    fn test_bincode_serialize_primitive() {
        let backend = BincodeBackend;
        let value: u64 = 1_000_000;

        let bytes = backend.serialize(&value).expect("serialize u64");
        let restored: u64 = backend.deserialize(&bytes).expect("deserialize u64");

        assert_eq!(value, restored);
    }

    #[test]
    fn test_bincode_serialize_string() {
        let backend = BincodeBackend;
        let value = "compact binary".to_string();

        let bytes = backend.serialize(&value).expect("serialize String");
        let restored: String = backend.deserialize(&bytes).expect("deserialize String");

        assert_eq!(value, restored);
    }

    #[test]
    fn test_bincode_serialize_struct() {
        let backend = BincodeBackend;
        let value = TestStruct {
            id: 42,
            name: "bincode test".to_string(),
            count: 1000,
        };

        let bytes = backend.serialize(&value).expect("serialize struct");
        let restored: TestStruct = backend.deserialize(&bytes).expect("deserialize struct");

        assert_eq!(value, restored);
    }

    #[test]
    fn test_bincode_serialize_vec() {
        let backend = BincodeBackend;
        let value: Vec<i32> = (-100..100).collect();

        let bytes = backend.serialize(&value).expect("serialize vec");
        let restored: Vec<i32> = backend.deserialize(&bytes).expect("deserialize vec");

        assert_eq!(value, restored);
    }
}

#[cfg(feature = "postcard-backend")]
mod postcard_tests {
    use qserde::backends::PostcardBackend;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct TestStruct {
        id: u32,
        name: String,
        flag: bool,
    }

    #[test]
    fn test_postcard_serialize_primitive() {
        let backend = PostcardBackend;
        let value: u8 = 255;

        let bytes = backend.serialize(&value).expect("serialize u8");
        let restored: u8 = backend.deserialize(&bytes).expect("deserialize u8");

        assert_eq!(value, restored);
    }

    #[test]
    fn test_postcard_serialize_string() {
        let backend = PostcardBackend;
        let value = "postcard compact".to_string();

        let bytes = backend.serialize(&value).expect("serialize String");
        let restored: String = backend.deserialize(&bytes).expect("deserialize String");

        assert_eq!(value, restored);
    }

    #[test]
    fn test_postcard_serialize_struct() {
        let backend = PostcardBackend;
        let value = TestStruct {
            id: 77,
            name: "embedded".to_string(),
            flag: true,
        };

        let bytes = backend.serialize(&value).expect("serialize struct");
        let restored: TestStruct = backend.deserialize(&bytes).expect("deserialize struct");

        assert_eq!(value, restored);
    }

    #[test]
    fn test_postcard_serialize_empty() {
        let backend = PostcardBackend;

        // Empty string
        let value = "".to_string();
        let bytes = backend.serialize(&value).expect("serialize empty string");
        let restored: String = backend
            .deserialize(&bytes)
            .expect("deserialize empty string");
        assert_eq!(value, restored);

        // Empty vec
        let value: Vec<u8> = vec![];
        let bytes = backend.serialize(&value).expect("serialize empty vec");
        let restored: Vec<u8> = backend.deserialize(&bytes).expect("deserialize empty vec");
        assert_eq!(value, restored);
    }
}

// Integration test: cross-backend compatibility (if multiple backends enabled)
#[cfg(all(feature = "serde-backend", feature = "cbor-backend"))]
mod cross_backend {
    use qserde::backends::{CborBackend, SerdeJsonBackend};

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct TestData {
        value: i32,
        name: String,
    }

    #[test]
    fn test_different_backends_produce_different_output() {
        let json_backend = SerdeJsonBackend;
        let cbor_backend = CborBackend;

        let data = TestData {
            value: 42,
            name: "cross".to_string(),
        };

        let json_bytes = json_backend.serialize(&data).expect("json serialize");
        let cbor_bytes = cbor_backend.serialize(&data).expect("cbor serialize");

        // Same data, different formats -> different byte lengths
        assert_ne!(json_bytes.len(), cbor_bytes.len());
    }
}
