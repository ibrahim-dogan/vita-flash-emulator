//! The map of property names to values used by the ActionScript VM.
//! This allows for dynamically choosing case-sensitivity at runtime,
//! because SWFv6 and below is case-insensitive. This also maintains
//! the insertion order of properties, which is necessary for accurate
//! enumeration order.

use crate::string::{utils as string_utils, AvmString, Units, WStr, WString};
use fnv::FnvBuildHasher;
use gc_arena::collect::Trace;
use gc_arena::Collect;
use indexmap::{Equivalent, IndexMap};
use std::hash::{Hash, Hasher};

type FnvIndexMap<K, V> = IndexMap<K, V, FnvBuildHasher>;

/// A map from property names to values.
#[derive(Default, Clone, Debug)]
pub struct PropertyMap<'gc, V>(FnvIndexMap<PropertyName<'gc>, V>);

impl<'gc, V> PropertyMap<'gc, V> {
    pub fn new() -> Self {
        Self(FnvIndexMap::default())
    }

    pub fn contains_key<T: PropertyKey>(&self, key: T, case_sensitive: bool) -> bool {
        if case_sensitive {
            self.0.contains_key(&CaseSensitive::new(&key))
        } else {
            self.0.contains_key(&CaseInsensitive::new(&key))
        }
    }

    pub fn entry<'a>(&'a mut self, key: AvmString<'gc>, case_sensitive: bool) -> Entry<'gc, 'a, V> {
        if case_sensitive {
            match self.0.get_index_of(&CaseSensitive::new(&key)) {
                Some(index) => Entry::Occupied(OccupiedEntry {
                    map: &mut self.0,
                    index,
                }),
                None => Entry::Vacant(VacantEntry {
                    map: &mut self.0,
                    key,
                }),
            }
        } else {
            match self.0.get_index_of(&CaseInsensitive::new(&key)) {
                Some(index) => Entry::Occupied(OccupiedEntry {
                    map: &mut self.0,
                    index,
                }),
                None => Entry::Vacant(VacantEntry {
                    map: &mut self.0,
                    key,
                }),
            }
        }
    }

    /// Gets the value for the specified property.
    pub fn get<T: PropertyKey>(&self, key: T, case_sensitive: bool) -> Option<&V> {
        if case_sensitive {
            self.0.get(&CaseSensitive::new(&key))
        } else {
            self.0.get(&CaseInsensitive::new(&key))
        }
    }

    /// Gets a mutable reference to the value for the specified property.
    pub fn get_mut<T: PropertyKey>(&mut self, key: T, case_sensitive: bool) -> Option<&mut V> {
        if case_sensitive {
            self.0.get_mut(&CaseSensitive::new(&key))
        } else {
            self.0.get_mut(&CaseInsensitive::new(&key))
        }
    }

    /// Gets a value by index, based on insertion order.
    pub fn get_index(&self, index: usize) -> Option<&V> {
        self.0.get_index(index).map(|(_, v)| v)
    }

    pub fn insert(&mut self, key: AvmString<'gc>, value: V, case_sensitive: bool) -> Option<V> {
        match self.entry(key, case_sensitive) {
            Entry::Occupied(entry) => Some(entry.insert(value)),
            Entry::Vacant(entry) => {
                entry.insert(value);
                None
            }
        }
    }

    /// Returns the value tuples in Flash's iteration order (most recently added first).
    pub fn iter(&self) -> impl Iterator<Item = (AvmString<'gc>, &V)> {
        self.0.iter().rev().map(|(k, v)| (k.0, v))
    }

    /// Returns the key-value tuples in Flash's iteration order (most recently added first).
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (AvmString<'gc>, &mut V)> {
        self.0.iter_mut().rev().map(|(k, v)| (k.0, v))
    }

    pub fn remove<T: PropertyKey>(&mut self, key: T, case_sensitive: bool) -> Option<V> {
        // Note that we must use shift_remove to maintain order in case this object is enumerated.
        if case_sensitive {
            self.0.shift_remove(&CaseSensitive::new(&key))
        } else {
            self.0.shift_remove(&CaseInsensitive::new(&key))
        }
    }
}

unsafe impl<'gc, V: Collect<'gc>> Collect<'gc> for PropertyMap<'gc, V> {
    fn trace<C: Trace<'gc>>(&self, cc: &mut C) {
        for (key, value) in &self.0 {
            cc.trace(key);
            cc.trace(value);
        }
    }
}

pub enum Entry<'gc, 'a, V> {
    Occupied(OccupiedEntry<'gc, 'a, V>),
    Vacant(VacantEntry<'gc, 'a, V>),
}

pub struct OccupiedEntry<'gc, 'a, V> {
    map: &'a mut FnvIndexMap<PropertyName<'gc>, V>,
    index: usize,
}

impl<'gc, V> OccupiedEntry<'gc, '_, V> {
    pub fn remove_entry(&mut self) -> (AvmString<'gc>, V) {
        let (k, v) = self.map.shift_remove_index(self.index).unwrap();
        (k.0, v)
    }

    pub fn get(&self) -> &V {
        self.map.get_index(self.index).unwrap().1
    }

    pub fn get_mut(&mut self) -> &mut V {
        self.map.get_index_mut(self.index).unwrap().1
    }

    pub fn insert(self, value: V) -> V {
        std::mem::replace(self.map.get_index_mut(self.index).unwrap().1, value)
    }
}

pub struct VacantEntry<'gc, 'a, V> {
    map: &'a mut FnvIndexMap<PropertyName<'gc>, V>,
    key: AvmString<'gc>,
}

impl<V> VacantEntry<'_, '_, V> {
    pub fn insert(self, value: V) {
        self.map.insert(PropertyName(self.key), value);
    }
}

/// RuffleVita: a property name to look up, with its case-insensitive hash.
/// `AvmString`s (nearly every lookup) memoize the hash, which is otherwise
/// recomputed on every property access and for every object on the
/// prototype chain.
pub trait PropertyKey {
    fn key_str(&self) -> &WStr;
    fn key_hash(&self) -> u64;
}

impl PropertyKey for AvmString<'_> {
    #[inline]
    fn key_str(&self) -> &WStr {
        self.as_wstr()
    }

    #[inline]
    fn key_hash(&self) -> u64 {
        self.cached_hash(swf_hash_string_ignore_case)
    }
}

impl<T: PropertyKey + ?Sized> PropertyKey for &T {
    #[inline]
    fn key_str(&self) -> &WStr {
        (**self).key_str()
    }

    #[inline]
    fn key_hash(&self) -> u64 {
        (**self).key_hash()
    }
}

impl PropertyKey for WStr {
    #[inline]
    fn key_str(&self) -> &WStr {
        self
    }

    #[inline]
    fn key_hash(&self) -> u64 {
        swf_hash_string_ignore_case(self) | 1
    }
}

impl PropertyKey for WString {
    #[inline]
    fn key_str(&self) -> &WStr {
        self.as_wstr()
    }

    #[inline]
    fn key_hash(&self) -> u64 {
        swf_hash_string_ignore_case(self) | 1
    }
}

/// Wraps a str-like type, causing the hash map to use a case insensitive hash and equality.
struct CaseInsensitive<'a> {
    name: &'a WStr,
    hash: u64,
}

impl<'a> CaseInsensitive<'a> {
    #[inline]
    fn new<T: PropertyKey>(key: &'a T) -> Self {
        Self { name: key.key_str(), hash: key.key_hash() }
    }
}

impl Hash for CaseInsensitive<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.hash);
    }
}

impl<'gc> Equivalent<PropertyName<'gc>> for CaseInsensitive<'_> {
    fn equivalent(&self, key: &PropertyName<'gc>) -> bool {
        key.0.eq_ignore_case(self.name)
    }
}

/// Wraps an str-like type, causing the property map to use a case insensitive hash lookup,
/// but case sensitive equality.
struct CaseSensitive<'a> {
    name: &'a WStr,
    hash: u64,
}

impl<'a> CaseSensitive<'a> {
    #[inline]
    fn new<T: PropertyKey>(key: &'a T) -> Self {
        Self { name: key.key_str(), hash: key.key_hash() }
    }
}

impl Hash for CaseSensitive<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.hash);
    }
}

impl<'gc> Equivalent<PropertyName<'gc>> for CaseSensitive<'_> {
    fn equivalent(&self, key: &PropertyName<'gc>) -> bool {
        key.0 == self.name
    }
}

/// The property keys stored in the property map.
/// This uses a case insensitive hash to ensure that properties can be found in
/// SWFv6, which is case insensitive. The equality check is handled by the `Equivalent`
/// impls above, which allow it to be either case-sensitive or insensitive.
/// Note that the property of if key1 == key2 -> hash(key1) == hash(key2) still holds.
#[derive(Debug, Clone, PartialEq, Eq, Collect)]
#[collect(no_drop)]
struct PropertyName<'gc>(AvmString<'gc>);

impl Hash for PropertyName<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.0.key_hash());
    }
}

/// RuffleVita: the name folded into one `u64` with a multiply per code unit,
/// case-insensitive, and the same for the 8-bit and 16-bit representations of
/// a string. Callers set the low bit (see `AvmString::cached_hash`).
#[inline]
fn swf_hash_string_ignore_case(s: &WStr) -> u64 {
    const K: u64 = 0x517c_c1b7_2722_0a95;
    #[inline(always)]
    fn fold(hash: u64, unit: u16) -> u64 {
        let lower = if unit < 0x80 {
            u16::from((unit as u8).to_ascii_lowercase())
        } else {
            string_utils::swf_to_lowercase(unit)
        };
        (hash.rotate_left(5) ^ u64::from(lower)).wrapping_mul(K)
    }
    match s.units() {
        Units::Bytes(units) => units.iter().fold(0, |h, &u| fold(h, u16::from(u))),
        Units::Wide(units) => units.iter().fold(0, |h, &u| fold(h, u)),
    }
}
