use qserde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

#[test]
fn test_serialize_array() {
    let value: [u8; 4] = [1, 2, 3, 4];
    let bytes = value.serialize().expect("array should serialize");
    let restored: [u8; 4] = <[u8; 4]>::deserialize(&bytes).expect("array should deserialize");
    assert_eq!(restored, value);
}

#[test]
fn test_serialize_large_array() {
    let value: [u64; 128] = [42u64; 128];
    let bytes = value.serialize().expect("large array should serialize");
    let restored: [u64; 128] =
        <[u64; 128]>::deserialize(&bytes).expect("large array should deserialize");
    assert_eq!(restored, value);
}

#[test]
fn test_serialize_tuple() {
    let value = (1u32, "hello".to_string(), 3.14f64);
    let bytes = value.serialize().expect("tuple should serialize");
    let restored: (u32, String, f64) =
        <(u32, String, f64)>::deserialize(&bytes).expect("tuple should deserialize");
    assert_eq!(restored, value);
}

#[test]
fn test_serialize_nested_tuple() {
    let value = ((1, 2), (3, 4), (5, 6));
    let bytes = value.serialize().expect("nested tuple should serialize");
    let restored: ((i32, i32), (i32, i32), (i32, i32)) =
        <((i32, i32), (i32, i32), (i32, i32))>::deserialize(&bytes)
            .expect("nested tuple should deserialize");
    assert_eq!(restored, value);
}

#[test]
fn test_serialize_hashset() {
    let mut set = HashSet::new();
    set.insert(1);
    set.insert(2);
    set.insert(3);

    let bytes = set.serialize().expect("HashSet should serialize");
    let restored: HashSet<i32> = HashSet::deserialize(&bytes).expect("HashSet should deserialize");

    assert_eq!(restored.len(), 3);
    assert!(restored.contains(&1));
    assert!(restored.contains(&2));
    assert!(restored.contains(&3));
}

#[test]
fn test_serialize_btreeset() {
    let mut set = BTreeSet::new();
    set.insert("a".to_string());
    set.insert("b".to_string());
    set.insert("c".to_string());

    let bytes = set.serialize().expect("BTreeSet should serialize");
    let restored: BTreeSet<String> =
        BTreeSet::deserialize(&bytes).expect("BTreeSet should deserialize");

    assert_eq!(restored.len(), 3);
    assert!(restored.contains(&"a".to_string()));
}

#[test]
fn test_serialize_btreemap() {
    let mut map = BTreeMap::new();
    map.insert(1, "one".to_string());
    map.insert(2, "two".to_string());
    map.insert(3, "three".to_string());

    let bytes = map.serialize().expect("BTreeMap should serialize");
    let restored: BTreeMap<i32, String> =
        BTreeMap::deserialize(&bytes).expect("BTreeMap should deserialize");

    assert_eq!(restored.get(&1), Some(&"one".to_string()));
    assert_eq!(restored.get(&2), Some(&"two".to_string()));
}

#[test]
fn test_serialize_vecdeque() {
    let mut deque = VecDeque::new();
    deque.push_back(1);
    deque.push_back(2);
    deque.push_front(0);

    let bytes = deque.serialize().expect("VecDeque should serialize");
    let restored: VecDeque<i32> =
        VecDeque::deserialize(&bytes).expect("VecDeque should deserialize");

    assert_eq!(restored.len(), 3);
    assert_eq!(*restored.front().unwrap(), 0);
    assert_eq!(*restored.back().unwrap(), 2);
}

#[test]
fn test_serialize_range() {
    let range = 1..10;
    let bytes = range.serialize().expect("Range should serialize");
    let restored: std::ops::Range<i32> =
        <std::ops::Range<i32>>::deserialize(&bytes).expect("Range should deserialize");

    assert_eq!(restored.start, 1);
    assert_eq!(restored.end, 10);
}

#[test]
fn test_serialize_rangeinclusive() {
    let range = 1..=10;
    let bytes = range.serialize().expect("RangeInclusive should serialize");
    let restored: std::ops::RangeInclusive<i32> =
        <std::ops::RangeInclusive<i32>>::deserialize(&bytes)
            .expect("RangeInclusive should deserialize");

    assert_eq!(*restored.start(), 1);
    assert_eq!(*restored.end(), 10);
}

#[qserde::Archive]
#[derive(Debug, PartialEq)]
struct Database {
    users: HashMap<u64, User>,
    metadata: BTreeMap<String, String>,
}

#[qserde::Archive]
#[derive(Debug, PartialEq)]
struct User {
    id: u64,
    name: String,
    roles: HashSet<String>,
}

#[test]
fn test_complex_nested_structure() {
    let mut users = HashMap::new();
    let mut roles = HashSet::new();
    roles.insert("admin".to_string());
    roles.insert("user".to_string());

    users.insert(
        1,
        User {
            id: 1,
            name: "Alice".to_string(),
            roles,
        },
    );

    let mut metadata = BTreeMap::new();
    metadata.insert("version".to_string(), "1.0".to_string());
    metadata.insert("created".to_string(), "2024-01-01".to_string());

    let db = Database { users, metadata };

    let bytes = db.serialize().expect("Database should serialize");
    let restored: Database = Database::deserialize(&bytes).expect("Database should deserialize");

    assert_eq!(restored, db);
}

#[qserde::Archive]
#[derive(Debug, PartialEq)]
struct UnitStruct;

#[test]
fn test_unit_type() {
    let value = ();
    let bytes = value.serialize().expect("unit should serialize");
    let restored: () = <()>::deserialize(&bytes).expect("unit should deserialize");
    assert_eq!(restored, value);
}

#[test]
fn test_unit_struct() {
    let value = UnitStruct;
    let bytes = value.serialize().expect("UnitStruct should serialize");
    let restored: UnitStruct =
        UnitStruct::deserialize(&bytes).expect("UnitStruct should deserialize");
    assert_eq!(restored, value);
}

#[qserde::Archive]
#[derive(Debug, PartialEq, Clone, Copy)]
enum Color {
    Red,
    Green,
    Blue,
}

#[test]
fn test_clike_enum() {
    let value = Color::Blue;
    let bytes = value.serialize().expect("Color should serialize");
    let restored: Color = Color::deserialize(&bytes).expect("Color should deserialize");
    assert_eq!(restored, value);
}

#[qserde::Archive]
#[derive(Debug, PartialEq)]
struct Wrapper<T> {
    value: T,
}

#[test]
fn test_generic_wrapper() {
    let wrapper = Wrapper { value: 42u64 };
    let bytes = wrapper.serialize().expect("Wrapper<u64> should serialize");
    let restored: Wrapper<u64> =
        Wrapper::deserialize(&bytes).expect("Wrapper<u64> should deserialize");
    assert_eq!(restored, wrapper);

    let wrapper2 = Wrapper {
        value: "hello".to_string(),
    };
    let bytes2 = wrapper2
        .serialize()
        .expect("Wrapper<String> should serialize");
    let restored2: Wrapper<String> =
        Wrapper::deserialize(&bytes2).expect("Wrapper<String> should deserialize");
    assert_eq!(restored2, wrapper2);
}

#[test]
fn test_f32() {
    let value: f32 = 3.14159;
    let bytes = value.serialize().expect("f32 should serialize");
    let restored: f32 = f32::deserialize(&bytes).expect("f32 should deserialize");
    assert!((restored - value).abs() < f32::EPSILON);
}

#[test]
fn test_f64() {
    let value: f64 = std::f64::consts::PI;
    let bytes = value.serialize().expect("f64 should serialize");
    let restored: f64 = f64::deserialize(&bytes).expect("f64 should deserialize");
    assert!((restored - value).abs() < f64::EPSILON);
}

#[test]
fn test_box() {
    let value = Box::new(42u64);
    let bytes = value.serialize().expect("Box should serialize");
    let restored: Box<u64> = Box::deserialize(&bytes).expect("Box should deserialize");
    assert_eq!(*restored, 42);
}

#[test]
fn test_rc() {
    use std::rc::Rc;

    let value = Rc::new(42u64);
    let bytes = value.serialize().expect("Rc should serialize");
    let restored: Rc<u64> = Rc::deserialize(&bytes).expect("Rc should deserialize");
    assert_eq!(*restored, 42);
}

#[test]
fn test_arc() {
    use std::sync::Arc;

    let value = Arc::new(42u64);
    let bytes = value.serialize().expect("Arc should serialize");
    let restored: Arc<u64> = Arc::deserialize(&bytes).expect("Arc should deserialize");
    assert_eq!(*restored, 42);
}

use std::marker::PhantomData;

#[qserde::Archive]
#[derive(Debug, PartialEq)]
struct PhantomWrapper<T> {
    value: u64,
    _marker: PhantomData<T>,
}

#[test]
fn test_phantom_data() {
    let value = PhantomWrapper::<String> {
        value: 42,
        _marker: PhantomData,
    };
    let bytes = value.serialize().expect("PhantomWrapper should serialize");
    let restored: PhantomWrapper<String> =
        PhantomWrapper::deserialize(&bytes).expect("PhantomWrapper should deserialize");
    assert_eq!(restored, value);
}
