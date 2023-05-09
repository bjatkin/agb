//! A lot of the documentation for this module was copied straight out of the rust
//! standard library. The implementation however is not.
#![no_std]
#![feature(allocator_api)]
#![deny(clippy::all)]
#![deny(clippy::must_use_candidate)]
#![deny(missing_docs)]
#![deny(clippy::trivially_copy_pass_by_ref)]
#![deny(clippy::semicolon_if_nothing_returned)]
#![deny(clippy::map_unwrap_or)]
#![deny(clippy::needless_pass_by_value)]
#![deny(clippy::redundant_closure_for_method_calls)]
#![deny(clippy::cloned_instead_of_copied)]
#![deny(rustdoc::broken_intra_doc_links)]
#![deny(rustdoc::private_intra_doc_links)]
#![deny(rustdoc::invalid_html_tags)]

extern crate alloc;

use alloc::alloc::Global;
use core::{
    alloc::Allocator,
    borrow::Borrow,
    hash::{BuildHasher, BuildHasherDefault, Hash, Hasher},
    iter::FromIterator,
    ops::Index,
};

use rustc_hash::FxHasher;

mod node;
mod node_storage;

use node::Node;
use node_storage::NodeStorage;

type HashType = u32;

// # Robin Hood Hash Tables
//
// The problem with regular hash tables where failing to find a slot for a specific
// key will result in a linear search for the first free slot is that often these
// slots can end up being quite far away from the original chosen location in fuller
// hash tables. In Java, the hash table will resize when it is more than 2 thirds full
// which is quite wasteful in terms of space. Robin Hood hash tables can be much
// fuller before needing to resize and also keeps search times lower.
//
// The key concept is to keep the distance from the initial bucket chosen for a given
// key to a minimum. We shall call this distance the "distance to the initial bucket"
// or DIB for short. With each key - value pair, we store its DIB. When inserting
// a value into the hash table, we check to see if there is an element in the initial
// bucket. If there is, we move onto the next value. Then, we check to see if there
// is already a value there and if there is, we check its DIB. If our DIB is greater
// than or equal to the DIB of the value that is already there, we swap the working
// value and the current entry. This continues until an empty slot is found.
//
// Using this technique, the average DIB is kept fairly low which decreases search
// times. As a simple search time optimisation, the maximum DIB is kept track of
// and so we will only need to search as far as that in order to know whether or
// not a given element is in the hash table.
//
// # Deletion
//
// Special mention is given to deletion. Unfortunately, the maximum DIB is not
// kept track of after deletion, since we would not only need to keep track of
// the maximum DIB but also the number of elements which have that maximum DIB.
//
// In order to delete an element, we search to see if it exists. If it does,
// we remove that element and then iterate through the array from that point
// and move each element back one space (updating its DIB). If the DIB of the
// element we are trying to remove is 0, then we stop this algorithm.
//
// This means that deletion will lower the average DIB of the elements and
// keep searching and insertion fast.
//
// # Rehashing
//
// Currently, no incremental rehashing takes place. Once the HashMap becomes
// more than 85% full (this value may change when I do some benchmarking),
// a new list is allocated with double the capacity and the entire node list
// is migrated.

/// A hash map implemented very simply using robin hood hashing.
///
/// `HashMap` uses `FxHasher` internally, which is a very fast hashing algorithm used
/// by rustc and firefox in non-adversarial places. It is incredibly fast, and good
/// enough for most cases.
///
/// It is required that the keys implement the [`Eq`] and [`Hash`] traits, although this
/// can be frequently achieved by using `#[derive(PartialEq, Eq, Hash)]`. If you
/// implement these yourself, it is important that the following property holds:
///
/// `k1 == k2 => hash(k1) == hash(k2)`
///
/// It is a logic error for the key to be modified in such a way that the key's hash, as
/// determined by the [`Hash`] trait, or its equality as determined by the [`Eq`] trait,
/// changes while it is in the map. The behaviour for such a logic error is not specified,
/// but will not result in undefined behaviour. This could include panics, incorrect results,
/// aborts, memory leaks and non-termination.
///
/// The API surface provided is incredibly similar to the
/// [`std::collections::HashMap`](https://doc.rust-lang.org/std/collections/struct.HashMap.html)
/// implementation with fewer guarantees, and better optimised for the GameBoy Advance.
///
/// [`Eq`]: https://doc.rust-lang.org/core/cmp/trait.Eq.html
/// [`Hash`]: https://doc.rust-lang.org/core/hash/trait.Hash.html
///
/// # Example
/// ```
/// use agb_hashmap::HashMap;
///
/// // Type inference lets you omit the type signature (which would be HashMap<String, String> in this example)
/// let mut game_reviews = HashMap::new();
///
/// // Review some games
/// game_reviews.insert(
///     "Pokemon Emerald".to_string(),
///     "Best post-game battle experience of any generation.".to_string(),
/// );
/// game_reviews.insert(
///     "Golden Sun".to_string(),
///     "Some of the best music on the console".to_string(),
/// );
/// game_reviews.insert(
///     "Super Dodge Ball Advance".to_string(),
///     "Really great launch title".to_string(),
/// );
///
/// // Check for a specific entry
/// if !game_reviews.contains_key("Legend of Zelda: The Minish Cap") {
///     println!("We've got {} reviews, but The Minish Cap ain't one", game_reviews.len());
/// }
///
/// // Iterate over everything
/// for (game, review) in &game_reviews {
///     println!("{game}: \"{review}\"");
/// }
/// ```
#[derive(Clone)]
pub struct HashMap<K, V, ALLOCATOR: Allocator = Global> {
    nodes: NodeStorage<K, V, ALLOCATOR>,

    hasher: BuildHasherDefault<FxHasher>,
}

/// Trait for allocators that are clonable, blanket implementation for all types that implement Allocator and Clone
pub trait ClonableAllocator: Allocator + Clone {}
impl<T: Allocator + Clone> ClonableAllocator for T {}

impl<K, V> HashMap<K, V> {
    /// Creates a `HashMap`
    #[must_use]
    pub fn new() -> Self {
        Self::new_in(Global)
    }

    /// Creates an empty `HashMap` with specified internal size. The size must be a power of 2
    #[must_use]
    pub fn with_size(size: usize) -> Self {
        Self::with_size_in(size, Global)
    }

    /// Creates an empty `HashMap` which can hold at least `capacity` elements before resizing. The actual
    /// internal size may be larger as it must be a power of 2
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_capacity_in(capacity, Global)
    }
}

impl<K, V, ALLOCATOR: ClonableAllocator> HashMap<K, V, ALLOCATOR> {
    #[must_use]
    /// Creates an empty `HashMap` with specified internal size using the
    /// specified allocator. The size must be a power of 2
    pub fn with_size_in(size: usize, alloc: ALLOCATOR) -> Self {
        Self {
            nodes: NodeStorage::with_size_in(size, alloc),
            hasher: Default::default(),
        }
    }

    #[must_use]
    /// Creates a `HashMap` with a specified allocator
    pub fn new_in(alloc: ALLOCATOR) -> Self {
        Self::with_size_in(16, alloc)
    }

    /// Returns a reference to the underlying allocator
    pub fn allocator(&self) -> &ALLOCATOR {
        self.nodes.allocator()
    }

    /// Creates an empty `HashMap` which can hold at least `capacity` elements before resizing. The actual
    /// internal size may be larger as it must be a power of 2
    #[must_use]
    pub fn with_capacity_in(capacity: usize, alloc: ALLOCATOR) -> Self {
        for i in 0..32 {
            let attempted_size = 1usize << i;
            if number_before_resize(attempted_size) > capacity {
                return Self::with_size_in(attempted_size, alloc);
            }
        }

        panic!(
            "Failed to come up with a size which satisfies capacity {}",
            capacity
        );
    }

    /// Returns the number of elements in the map
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns the number of elements the map can hold
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.nodes.capacity()
    }

    /// An iterator visiting all keys in an arbitrary order
    pub fn keys(&self) -> impl Iterator<Item = &'_ K> {
        self.iter().map(|(k, _)| k)
    }

    /// An iterator visiting all values in an arbitrary order
    pub fn values(&self) -> impl Iterator<Item = &'_ V> {
        self.iter().map(|(_, v)| v)
    }

    /// An iterator visiting all values in an arbitrary order allowing for mutation
    pub fn values_mut(&mut self) -> impl Iterator<Item = &'_ mut V> {
        self.iter_mut().map(|(_, v)| v)
    }

    /// Removes all elements from the map
    pub fn clear(&mut self) {
        self.nodes =
            NodeStorage::with_size_in(self.nodes.backing_vec_size(), self.allocator().clone());
    }

    /// An iterator visiting all key-value pairs in an arbitrary order
    pub fn iter(&self) -> impl Iterator<Item = (&'_ K, &'_ V)> {
        Iter {
            map: self,
            at: 0,
            num_found: 0,
        }
    }

    /// An iterator visiting all key-value pairs in an arbitrary order, with mutable references to the values
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&'_ K, &'_ mut V)> {
        self.nodes.iter_mut().filter_map(Node::key_value_mut)
    }

    /// Retains only the elements specified by the predicate `f`.
    pub fn retain<F>(&mut self, f: F)
    where
        F: FnMut(&K, &mut V) -> bool,
    {
        self.nodes.retain(f);
    }

    /// Returns `true` if the map contains no elements
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn resize(&mut self, new_size: usize) {
        assert!(
            new_size >= self.nodes.backing_vec_size(),
            "Can only increase the size of a hash map"
        );
        if new_size == self.nodes.backing_vec_size() {
            return;
        }

        self.nodes = self.nodes.resized_to(new_size);
    }
}

impl<K, V> Default for HashMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V, ALLOCATOR: ClonableAllocator> HashMap<K, V, ALLOCATOR>
where
    K: Eq + Hash,
{
    /// Inserts a key-value pair into the map.
    ///
    /// If the map did not have this key present, [`None`] is returned.
    ///
    /// If the map did have this key present, the value is updated and the old value
    /// is returned. The key is not updated, which matters for types that can be `==`
    /// without being identical.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let hash = self.hash(&key);

        if let Some(location) = self.nodes.location(&key, hash) {
            Some(self.nodes.replace_at_location(location, key, value))
        } else {
            if self.nodes.capacity() <= self.len() {
                self.resize(self.nodes.backing_vec_size() * 2);
            }

            self.nodes.insert_new(key, value, hash);

            None
        }
    }

    fn insert_and_get(&mut self, key: K, value: V) -> &'_ mut V {
        let hash = self.hash(&key);

        let location = if let Some(location) = self.nodes.location(&key, hash) {
            self.nodes.replace_at_location(location, key, value);
            location
        } else {
            if self.nodes.capacity() <= self.len() {
                self.resize(self.nodes.backing_vec_size() * 2);
            }

            self.nodes.insert_new(key, value, hash)
        };

        self.nodes.node_at_mut(location).value_mut().unwrap()
    }

    /// Returns `true` if the map contains a value for the specified key.
    pub fn contains_key<Q>(&self, k: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let hash = self.hash(k);
        self.nodes.location(k, hash).is_some()
    }

    /// Returns the key-value pair corresponding to the supplied key
    pub fn get_key_value<Q>(&self, key: &Q) -> Option<(&K, &V)>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let hash = self.hash(key);

        self.nodes
            .location(key, hash)
            .and_then(|location| self.nodes.node_at(location).key_value_ref())
    }

    /// Returns a reference to the value corresponding to the key. Returns [`None`] if there is
    /// no element in the map with the given key.
    ///
    /// # Example
    /// ```
    /// use agb_hashmap::HashMap;
    ///
    /// let mut map = HashMap::new();
    /// map.insert("a".to_string(), "A");
    /// assert_eq!(map.get("a"), Some(&"A"));
    /// assert_eq!(map.get("b"), None);
    /// ```
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.get_key_value(key).map(|(_, v)| v)
    }

    /// Returns a mutable reference to the value corresponding to the key. Return [`None`] if
    /// there is no element in the map with the given key.
    ///
    /// # Example
    /// ```
    /// use agb_hashmap::HashMap;
    ///
    /// let mut map = HashMap::new();
    /// map.insert("a".to_string(), "A");
    ///
    /// if let Some(x) = map.get_mut("a") {
    ///     *x = "b";
    /// }
    ///
    /// assert_eq!(map["a"], "b");
    /// ```
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let hash = self.hash(key);

        if let Some(location) = self.nodes.location(key, hash) {
            self.nodes.node_at_mut(location).value_mut()
        } else {
            None
        }
    }

    /// Removes the given key from the map. Returns the current value if it existed, or [`None`]
    /// if it did not.
    ///
    /// # Example
    /// ```
    /// use agb_hashmap::HashMap;
    ///
    /// let mut map = HashMap::new();
    /// map.insert(1, "a");
    /// assert_eq!(map.remove(&1), Some("a"));
    /// assert_eq!(map.remove(&1), None);
    /// ```
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let hash = self.hash(key);

        self.nodes
            .location(key, hash)
            .map(|location| self.nodes.remove_from_location(location))
    }
}

impl<K, V, ALLOCATOR: ClonableAllocator> HashMap<K, V, ALLOCATOR>
where
    K: Hash,
{
    fn hash<Q>(&self, key: &Q) -> HashType
    where
        K: Borrow<Q>,
        Q: Hash + ?Sized,
    {
        let mut hasher = self.hasher.build_hasher();
        key.hash(&mut hasher);
        hasher.finish() as HashType
    }
}

/// An iterator over entries of a [`HashMap`]
///
/// This struct is created using the `into_iter()` method on [`HashMap`]. See its
/// documentation for more.
pub struct Iter<'a, K: 'a, V: 'a, ALLOCATOR: ClonableAllocator> {
    map: &'a HashMap<K, V, ALLOCATOR>,
    at: usize,
    num_found: usize,
}

impl<'a, K, V, ALLOCATOR: ClonableAllocator> Iterator for Iter<'a, K, V, ALLOCATOR> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.at >= self.map.nodes.backing_vec_size() {
                return None;
            }

            let node = &self.map.nodes.node_at(self.at);
            self.at += 1;

            if node.has_value() {
                self.num_found += 1;
                return Some((node.key_ref().unwrap(), node.value_ref().unwrap()));
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (
            self.map.len() - self.num_found,
            Some(self.map.len() - self.num_found),
        )
    }
}

impl<'a, K, V, ALLOCATOR: ClonableAllocator> IntoIterator for &'a HashMap<K, V, ALLOCATOR> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V, ALLOCATOR>;

    fn into_iter(self) -> Self::IntoIter {
        Iter {
            map: self,
            at: 0,
            num_found: 0,
        }
    }
}

/// An iterator over entries of a [`HashMap`]
///
/// This struct is created using the `into_iter()` method on [`HashMap`] as part of its implementation
/// of the IntoIterator trait.
pub struct IterOwned<K, V, ALLOCATOR: Allocator = Global> {
    map: HashMap<K, V, ALLOCATOR>,
    at: usize,
    num_found: usize,
}

impl<K, V, ALLOCATOR: ClonableAllocator> Iterator for IterOwned<K, V, ALLOCATOR> {
    type Item = (K, V);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.at >= self.map.nodes.backing_vec_size() {
                return None;
            }

            let maybe_kv = self.map.nodes.node_at_mut(self.at).take_key_value();
            self.at += 1;

            if let Some((k, v, _)) = maybe_kv {
                self.num_found += 1;
                return Some((k, v));
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (
            self.map.len() - self.num_found,
            Some(self.map.len() - self.num_found),
        )
    }
}

/// An iterator over entries of a [`HashMap`]
///
/// This struct is created using the `into_iter()` method on [`HashMap`] as part of its implementation
/// of the IntoIterator trait.
impl<K, V, ALLOCATOR: ClonableAllocator> IntoIterator for HashMap<K, V, ALLOCATOR> {
    type Item = (K, V);
    type IntoIter = IterOwned<K, V, ALLOCATOR>;

    fn into_iter(self) -> Self::IntoIter {
        IterOwned {
            map: self,
            at: 0,
            num_found: 0,
        }
    }
}

/// A view into an occupied entry in a `HashMap`. This is part of the [`Entry`] enum.
pub struct OccupiedEntry<'a, K: 'a, V: 'a, ALLOCATOR: Allocator> {
    key: K,
    map: &'a mut HashMap<K, V, ALLOCATOR>,
    location: usize,
}

impl<'a, K: 'a, V: 'a, ALLOCATOR: ClonableAllocator> OccupiedEntry<'a, K, V, ALLOCATOR> {
    /// Gets a reference to the key in the entry.
    pub fn key(&self) -> &K {
        &self.key
    }

    /// Take the ownership of the key and value from the map.
    pub fn remove_entry(self) -> (K, V) {
        let old_value = self.map.nodes.remove_from_location(self.location);
        (self.key, old_value)
    }

    /// Gets a reference to the value in the entry.
    pub fn get(&self) -> &V {
        self.map.nodes.node_at(self.location).value_ref().unwrap()
    }

    /// Gets a mutable reference to the value in the entry.
    ///
    /// If you need a reference to the `OccupiedEntry` which may outlive the destruction
    /// of the `Entry` value, see [`into_mut`].
    ///
    /// [`into_mut`]: Self::into_mut
    pub fn get_mut(&mut self) -> &mut V {
        self.map
            .nodes
            .node_at_mut(self.location)
            .value_mut()
            .unwrap()
    }

    /// Converts the `OccupiedEntry` into a mutable reference to the value in the entry with
    /// a lifetime bound to the map itself.
    ///
    /// If you need multiple references to the `OccupiedEntry`, see [`get_mut`].
    ///
    /// [`get_mut`]: Self::get_mut
    pub fn into_mut(self) -> &'a mut V {
        self.map
            .nodes
            .node_at_mut(self.location)
            .value_mut()
            .unwrap()
    }

    /// Sets the value of the entry and returns the entry's old value.
    pub fn insert(&mut self, value: V) -> V {
        self.map
            .nodes
            .node_at_mut(self.location)
            .replace_value(value)
    }

    /// Takes the value out of the entry and returns it.
    pub fn remove(self) -> V {
        self.map.nodes.remove_from_location(self.location)
    }
}

/// A view into a vacant entry in a `HashMap`. It is part of the [`Entry`] enum.
pub struct VacantEntry<'a, K: 'a, V: 'a, ALLOCATOR: Allocator> {
    key: K,
    map: &'a mut HashMap<K, V, ALLOCATOR>,
}

impl<'a, K: 'a, V: 'a, ALLOCATOR: ClonableAllocator> VacantEntry<'a, K, V, ALLOCATOR> {
    /// Gets a reference to the key that would be used when inserting a value through `VacantEntry`
    pub fn key(&self) -> &K {
        &self.key
    }

    /// Take ownership of the key
    pub fn into_key(self) -> K {
        self.key
    }

    /// Sets the value of the entry with the `VacantEntry`'s key and returns a mutable reference to it.
    pub fn insert(self, value: V) -> &'a mut V
    where
        K: Hash + Eq,
    {
        self.map.insert_and_get(self.key, value)
    }
}

/// A view into a single entry in a map, which may be vacant or occupied.
///
/// This is constructed using the [`entry`] method on [`HashMap`]
///
/// [`entry`]: HashMap::entry()
pub enum Entry<'a, K: 'a, V: 'a, ALLOCATOR: Allocator = Global> {
    /// An occupied entry
    Occupied(OccupiedEntry<'a, K, V, ALLOCATOR>),
    /// A vacant entry
    Vacant(VacantEntry<'a, K, V, ALLOCATOR>),
}

impl<'a, K, V, ALLOCATOR: ClonableAllocator> Entry<'a, K, V, ALLOCATOR>
where
    K: Hash + Eq,
{
    /// Ensures a value is in the entry by inserting the given value, and returns a mutable
    /// reference to the value in the entry.
    pub fn or_insert(self, value: V) -> &'a mut V {
        match self {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => e.insert(value),
        }
    }

    /// Ensures a value is in the entry by inserting the result of the function if empty, and
    /// returns a mutable reference to the value in the entry.
    pub fn or_insert_with<F>(self, f: F) -> &'a mut V
    where
        F: FnOnce() -> V,
    {
        match self {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => e.insert(f()),
        }
    }

    /// Ensures a value is in the entry by inserting the result of the function if empty, and
    /// returns a mutable reference to the value in the entry. This method allows for key-derived
    /// values for insertion by providing the function with a reference to the key.
    ///
    /// The reference to the moved key is provided so that cloning or copying the key is unnecessary,
    /// unlike with `.or_insert_with(|| ...)`.
    pub fn or_insert_with_key<F>(self, f: F) -> &'a mut V
    where
        F: FnOnce(&K) -> V,
    {
        match self {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => {
                let value = f(&e.key);
                e.insert(value)
            }
        }
    }

    /// Provides in-place mutable access to an occupied entry before any potential inserts
    /// into the map.
    pub fn and_modify<F>(self, f: F) -> Self
    where
        F: FnOnce(&mut V),
    {
        match self {
            Entry::Occupied(mut e) => {
                f(e.get_mut());
                Entry::Occupied(e)
            }
            Entry::Vacant(e) => Entry::Vacant(e),
        }
    }

    /// Ensures a value is in th entry by inserting the default value if empty. Returns a
    /// mutable reference to the value in the entry.
    pub fn or_default(self) -> &'a mut V
    where
        V: Default,
    {
        match self {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => e.insert(Default::default()),
        }
    }

    /// Returns a reference to this entry's key.
    pub fn key(&self) -> &K {
        match self {
            Entry::Occupied(e) => &e.key,
            Entry::Vacant(e) => &e.key,
        }
    }
}

impl<K, V, ALLOCATOR: ClonableAllocator> HashMap<K, V, ALLOCATOR>
where
    K: Hash + Eq,
{
    /// Gets the given key's corresponding entry in the map for in-place manipulation.
    pub fn entry(&mut self, key: K) -> Entry<'_, K, V, ALLOCATOR> {
        let hash = self.hash(&key);
        let location = self.nodes.location(&key, hash);

        if let Some(location) = location {
            Entry::Occupied(OccupiedEntry {
                key,
                location,
                map: self,
            })
        } else {
            Entry::Vacant(VacantEntry { key, map: self })
        }
    }
}

impl<K, V> FromIterator<(K, V)> for HashMap<K, V>
where
    K: Eq + Hash,
{
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let mut map = HashMap::new();
        map.extend(iter);
        map
    }
}

impl<K, V> Extend<(K, V)> for HashMap<K, V>
where
    K: Eq + Hash,
{
    fn extend<T: IntoIterator<Item = (K, V)>>(&mut self, iter: T) {
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

impl<K, V, Q, ALLOCATOR: ClonableAllocator> Index<&Q> for HashMap<K, V, ALLOCATOR>
where
    K: Eq + Hash + Borrow<Q>,
    Q: Eq + Hash + ?Sized,
{
    type Output = V;

    fn index(&self, key: &Q) -> &V {
        self.get(key).expect("no entry found for key")
    }
}

impl<K, V, ALLOCATOR: ClonableAllocator> PartialEq for HashMap<K, V, ALLOCATOR>
where
    K: Eq + Hash,
    V: PartialEq,
{
    fn eq(&self, other: &HashMap<K, V, ALLOCATOR>) -> bool {
        if self.len() != other.len() {
            return false;
        }

        self.iter()
            .all(|(key, value)| other.get(key).map_or(false, |v| *value == *v))
    }
}

impl<K, V, ALLOCATOR: ClonableAllocator> Eq for HashMap<K, V, ALLOCATOR>
where
    K: Eq + Hash,
    V: PartialEq,
{
}

const fn number_before_resize(capacity: usize) -> usize {
    capacity * 85 / 100
}

#[cfg(test)]
mod test {
    use core::cell::RefCell;

    use alloc::vec::Vec;

    use super::*;

    #[test]
    fn can_store_and_retrieve_8_elements() {
        let mut map = HashMap::new();

        for i in 0..8 {
            map.insert(i, i % 4);
        }

        for i in 0..8 {
            assert_eq!(map.get(&i), Some(&(i % 4)));
        }
    }

    #[test]
    fn can_get_the_length() {
        let mut map = HashMap::new();

        for i in 0..8 {
            map.insert(i / 2, true);
        }

        assert_eq!(map.len(), 4);
    }

    #[test]
    fn returns_none_if_element_does_not_exist() {
        let mut map = HashMap::new();

        for i in 0..8 {
            map.insert(i, i % 3);
        }

        assert_eq!(map.get(&12), None);
    }

    #[test]
    fn can_delete_entries() {
        let mut map = HashMap::new();

        for i in 0..8 {
            map.insert(i, i % 3);
        }

        for i in 0..4 {
            map.remove(&i);
        }

        assert_eq!(map.len(), 4);
        assert_eq!(map.get(&3), None);
        assert_eq!(map.get(&7), Some(&1));
    }

    #[test]
    fn can_iterate_through_all_entries() {
        let mut map = HashMap::new();

        for i in 0..8 {
            map.insert(i, i);
        }

        let mut max_found = -1;
        let mut num_found = 0;

        for (_, value) in map.into_iter() {
            max_found = max_found.max(value);
            num_found += 1;
        }

        assert_eq!(num_found, 8);
        assert_eq!(max_found, 7);
    }

    #[test]
    fn can_insert_more_than_initial_capacity() {
        let mut map = HashMap::new();

        for i in 0..65 {
            map.insert(i, i % 4);
        }

        for i in 0..65 {
            assert_eq!(map.get(&i), Some(&(i % 4)));
        }
    }

    struct NoisyDrop {
        i: i32,
        dropped: bool,
    }

    impl NoisyDrop {
        #[cfg(not(miri))]
        fn new(i: i32) -> Self {
            Self { i, dropped: false }
        }
    }

    impl PartialEq for NoisyDrop {
        fn eq(&self, other: &Self) -> bool {
            self.i == other.i
        }
    }

    impl Eq for NoisyDrop {}

    impl Hash for NoisyDrop {
        fn hash<H: Hasher>(&self, hasher: &mut H) {
            hasher.write_i32(self.i);
        }
    }

    impl Drop for NoisyDrop {
        fn drop(&mut self) {
            if self.dropped {
                panic!("NoisyDropped dropped twice");
            }

            self.dropped = true;
        }
    }

    trait RngNextI32 {
        fn next_i32(&mut self) -> i32;
    }

    impl<T> RngNextI32 for T
    where
        T: rand::RngCore,
    {
        fn next_i32(&mut self) -> i32 {
            self.next_u32() as i32
        }
    }

    #[cfg(not(miri))] // takes way too long to run under miri
    #[test]
    fn extreme_case() {
        use rand::SeedableRng;

        let mut map = HashMap::new();
        let mut rng = rand::rngs::SmallRng::seed_from_u64(20);

        let mut answers: [Option<i32>; 128] = [None; 128];

        for _ in 0..5_000 {
            let command = rng.next_i32().rem_euclid(2);
            let key = rng.next_i32().rem_euclid(answers.len() as i32);
            let value = rng.next_i32();

            match command {
                0 => {
                    // insert
                    answers[key as usize] = Some(value);
                    map.insert(NoisyDrop::new(key), NoisyDrop::new(value));
                }
                1 => {
                    // remove
                    answers[key as usize] = None;
                    map.remove(&NoisyDrop::new(key));
                }
                _ => {}
            }

            for (i, answer) in answers.iter().enumerate() {
                assert_eq!(
                    map.get(&NoisyDrop::new(i as i32)).map(|nd| &nd.i),
                    answer.as_ref()
                );
            }
        }
    }

    #[derive(Clone)]
    struct Droppable<'a> {
        id: usize,
        drop_registry: &'a DropRegistry,
    }

    impl Hash for Droppable<'_> {
        fn hash<H: Hasher>(&self, hasher: &mut H) {
            hasher.write_usize(self.id);
        }
    }

    impl PartialEq for Droppable<'_> {
        fn eq(&self, other: &Self) -> bool {
            self.id == other.id
        }
    }

    impl Eq for Droppable<'_> {}

    impl Drop for Droppable<'_> {
        fn drop(&mut self) {
            self.drop_registry.dropped(self.id);
        }
    }

    struct DropRegistry {
        are_dropped: RefCell<Vec<i32>>,
    }

    impl DropRegistry {
        pub fn new() -> Self {
            Self {
                are_dropped: Default::default(),
            }
        }

        pub fn new_droppable(&self) -> Droppable<'_> {
            self.are_dropped.borrow_mut().push(0);
            Droppable {
                id: self.are_dropped.borrow().len() - 1,
                drop_registry: self,
            }
        }

        pub fn dropped(&self, id: usize) {
            self.are_dropped.borrow_mut()[id] += 1;
        }

        pub fn assert_dropped_once(&self, id: usize) {
            assert_eq!(self.are_dropped.borrow()[id], 1);
        }

        pub fn assert_not_dropped(&self, id: usize) {
            assert_eq!(self.are_dropped.borrow()[id], 0);
        }

        pub fn assert_dropped_n_times(&self, id: usize, num_drops: i32) {
            assert_eq!(self.are_dropped.borrow()[id], num_drops);
        }
    }

    #[test]
    fn correctly_drops_on_remove_and_overall_drop() {
        let drop_registry = DropRegistry::new();

        let droppable1 = drop_registry.new_droppable();
        let droppable2 = drop_registry.new_droppable();

        let id1 = droppable1.id;
        let id2 = droppable2.id;

        {
            let mut map = HashMap::new();

            map.insert(1, droppable1);
            map.insert(2, droppable2);

            drop_registry.assert_not_dropped(id1);
            drop_registry.assert_not_dropped(id2);

            map.remove(&1);
            drop_registry.assert_dropped_once(id1);
            drop_registry.assert_not_dropped(id2);
        }

        drop_registry.assert_dropped_once(id2);
    }

    #[test]
    fn correctly_drop_on_override() {
        let drop_registry = DropRegistry::new();

        let droppable1 = drop_registry.new_droppable();
        let droppable2 = drop_registry.new_droppable();

        let id1 = droppable1.id;
        let id2 = droppable2.id;

        {
            let mut map = HashMap::new();

            map.insert(1, droppable1);
            drop_registry.assert_not_dropped(id1);
            map.insert(1, droppable2);

            drop_registry.assert_dropped_once(id1);
            drop_registry.assert_not_dropped(id2);
        }

        drop_registry.assert_dropped_once(id2);
    }

    #[test]
    fn correctly_drops_key_on_override() {
        let drop_registry = DropRegistry::new();

        let droppable1 = drop_registry.new_droppable();
        let droppable1a = droppable1.clone();

        let id1 = droppable1.id;

        {
            let mut map = HashMap::new();

            map.insert(droppable1, 1);
            drop_registry.assert_not_dropped(id1);
            map.insert(droppable1a, 2);

            drop_registry.assert_dropped_once(id1);
        }

        drop_registry.assert_dropped_n_times(id1, 2);
    }

    #[test]
    fn test_retain() {
        let mut map = HashMap::new();

        for i in 0..100 {
            map.insert(i, i);
        }

        map.retain(|k, _| k % 2 == 0);

        assert_eq!(map[&2], 2);
        assert_eq!(map.get(&3), None);

        assert_eq!(map.iter().count(), 50); // force full iteration
    }

    #[test]
    fn test_size_hint_iter() {
        let mut map = HashMap::new();

        for i in 0..100 {
            map.insert(i, i);
        }

        let mut iter = map.iter();
        assert_eq!(iter.size_hint(), (100, Some(100)));

        iter.next();

        assert_eq!(iter.size_hint(), (99, Some(99)));
    }

    #[test]
    fn test_size_hint_into_iter() {
        let mut map = HashMap::new();

        for i in 0..100 {
            map.insert(i, i);
        }

        let mut iter = map.into_iter();
        assert_eq!(iter.size_hint(), (100, Some(100)));

        iter.next();

        assert_eq!(iter.size_hint(), (99, Some(99)));
    }

    // Following test cases copied from the rust source
    // https://github.com/rust-lang/rust/blob/master/library/std/src/collections/hash/map/tests.rs
    mod rust_std_tests {
        use crate::{Entry::*, HashMap};

        #[test]
        fn test_entry() {
            let xs = [(1, 10), (2, 20), (3, 30), (4, 40), (5, 50), (6, 60)];

            let mut map: HashMap<_, _> = xs.iter().copied().collect();

            // Existing key (insert)
            match map.entry(1) {
                Vacant(_) => unreachable!(),
                Occupied(mut view) => {
                    assert_eq!(view.get(), &10);
                    assert_eq!(view.insert(100), 10);
                }
            }
            assert_eq!(map.get(&1).unwrap(), &100);
            assert_eq!(map.len(), 6);

            // Existing key (update)
            match map.entry(2) {
                Vacant(_) => unreachable!(),
                Occupied(mut view) => {
                    let v = view.get_mut();
                    let new_v = (*v) * 10;
                    *v = new_v;
                }
            }
            assert_eq!(map.get(&2).unwrap(), &200);
            assert_eq!(map.len(), 6);

            // Existing key (take)
            match map.entry(3) {
                Vacant(_) => unreachable!(),
                Occupied(view) => {
                    assert_eq!(view.remove(), 30);
                }
            }
            assert_eq!(map.get(&3), None);
            assert_eq!(map.len(), 5);

            // Inexistent key (insert)
            match map.entry(10) {
                Occupied(_) => unreachable!(),
                Vacant(view) => {
                    assert_eq!(*view.insert(1000), 1000);
                }
            }
            assert_eq!(map.get(&10).unwrap(), &1000);
            assert_eq!(map.len(), 6);
        }

        #[test]
        fn test_occupied_entry_key() {
            let mut a = HashMap::new();
            let key = "hello there";
            let value = "value goes here";
            assert!(a.is_empty());
            a.insert(key, value);
            assert_eq!(a.len(), 1);
            assert_eq!(a[key], value);

            match a.entry(key) {
                Vacant(_) => panic!(),
                Occupied(e) => assert_eq!(key, *e.key()),
            }
            assert_eq!(a.len(), 1);
            assert_eq!(a[key], value);
        }

        #[test]
        fn test_vacant_entry_key() {
            let mut a = HashMap::new();
            let key = "hello there";
            let value = "value goes here";

            assert!(a.is_empty());
            match a.entry(key) {
                Occupied(_) => panic!(),
                Vacant(e) => {
                    assert_eq!(key, *e.key());
                    e.insert(value);
                }
            }
            assert_eq!(a.len(), 1);
            assert_eq!(a[key], value);
        }

        #[test]
        fn test_index() {
            let mut map = HashMap::new();

            map.insert(1, 2);
            map.insert(2, 1);
            map.insert(3, 4);

            assert_eq!(map[&2], 1);
        }
    }
}
