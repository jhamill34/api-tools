#![allow(
    clippy::arithmetic_side_effects,
    reason = "byte-index arithmetic for wildcard matching, bounds-checked by the slice ops around it"
)]

//! A byte-wise trie for wildcard-aware string-key lookups — e.g. matching a
//! concrete key like `"application/json"` against a registered pattern like
//! `"application/*"`.

use std::collections::HashMap;

/// Configures which bytes act as wildcards when inserting into or looking up
/// keys in a [`Trie`].
#[non_exhaustive]
pub struct Config {
    /// Matches exactly one arbitrary byte at its position — e.g. with
    /// `b'x'`, the pattern `"2xx"` matches `"201"`.
    single_wildcard_byte: u8,

    /// Matches any remaining suffix of the key — e.g. with `b'*'`, the
    /// pattern `"application/*"` matches `"application/json"`.
    multi_wildcard_byte: u8,
}

impl Config {
    /// Builds a [`Config`] using `single_wildcard_byte` to match exactly one
    /// byte and `multi_wildcard_byte` to match any remaining suffix.
    #[inline]
    #[must_use]
    pub const fn new(single_wildcard_byte: u8, multi_wildcard_byte: u8) -> Self {
        Self {
            single_wildcard_byte,
            multi_wildcard_byte,
        }
    }
}

impl Default for Config {
    /// `b'x'` for the single-byte wildcard, `b'*'` for the multi-byte one.
    #[inline]
    fn default() -> Self {
        Self::new(b'x', b'*')
    }
}

/// A byte-wise trie mapping string keys to values of type `T`, supporting
/// the two wildcard bytes configured in [`Config`].
#[non_exhaustive]
pub struct Trie<'trie, T> {
    /// The value stored at this exact node, if a key ending here has been
    /// inserted.
    value: Option<T>,

    /// Child nodes keyed by the next byte of the key.
    children: HashMap<u8, Trie<'trie, T>>,

    /// The wildcard configuration shared by this node and all its
    /// descendants.
    config: &'trie Config,
}

impl<T> Default for Trie<'_, T> {
    #[inline]
    fn default() -> Self {
        Self::new(&Config {
            single_wildcard_byte: b'x',
            multi_wildcard_byte: b'*',
        })
    }
}

impl<'trie, T> Trie<'trie, T> {
    /// Creates an empty trie using the given wildcard `config`.
    #[inline]
    #[must_use]
    pub fn new(config: &'trie Config) -> Self {
        Self {
            value: None,
            children: HashMap::new(),
            config,
        }
    }

    /// Inserts `value` at `key`, overwriting any value already stored there.
    #[inline]
    pub fn insert(&mut self, key: &str, value: T) {
        let chars = key.as_bytes();
        self.insert_bytes(chars, value);
    }

    /// Recursive, byte-at-a-time implementation of [`insert`](Trie::insert).
    #[inline]
    fn insert_bytes(&mut self, key: &[u8], value: T) {
        if let Some(next) = key.first() {
            let child = self
                .children
                .entry(*next)
                .or_insert_with(|| Trie::new(self.config));

            let remainder = key.get(1..).unwrap_or_default();
            child.insert_bytes(remainder, value);
        } else {
            self.value = Some(value);
        }
    }

    /// Looks up the value stored for `key`, falling back to the nearest
    /// matching wildcard entry if there's no exact match.
    #[inline]
    pub fn find(&self, key: &str) -> Option<&T> {
        let chars = key.as_bytes();
        self.find_bytes(chars)
    }

    /// Recursive, byte-at-a-time implementation of [`find`](Trie::find).
    #[inline]
    fn find_bytes(&self, key: &[u8]) -> Option<&T> {
        let Some(next) = key.first() else {
            return self.value.as_ref();
        };
        let remainder = key.get(1..).unwrap_or_default();

        let found = self
            .children
            .get(next)
            .or_else(|| self.children.get(&self.config.single_wildcard_byte))
            .and_then(|child| child.find_bytes(remainder));

        if found.is_some() {
            found
        } else {
            self.find_multi_wildcard(key)
        }
    }

    /// Falls back to the multi-byte wildcard child (if any), matching it
    /// against the longest suffix of `key` that has a further child under
    /// it, or its own value if `key` is empty from that point on.
    #[inline]
    fn find_multi_wildcard(&self, key: &[u8]) -> Option<&T> {
        let wildcard = self.children.get(&self.config.multi_wildcard_byte)?;

        let wildcard_child = key
            .iter()
            .enumerate()
            .find_map(|(index, val)| wildcard.children.get(val).map(|child| (index, child)));

        let Some((index, wildcard_child)) = wildcard_child else {
            return wildcard.value.as_ref();
        };

        match key.get((index + 1)..) {
            Some(remainder) => wildcard_child.find_bytes(remainder),
            None => wildcard_child.value.as_ref(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn simple_test() {
        let mut root = Trie::<u32>::default();

        root.insert("*/*", 30);
        root.insert("application/json", 10);
        root.insert("application/*", 20);

        assert_eq!(None, root.find("app"));
        assert_eq!(None, root.find("applier"));
        assert_eq!(Some(10), root.find("application/json").copied());
        assert_eq!(Some(20), root.find("application/csv").copied());
        assert_eq!(Some(30), root.find("app/csv").copied());
    }

    #[test]
    fn test_wildcard() {
        let mut root = Trie::<u32>::default();

        root.insert("2xx", 20);

        assert_eq!(None, root.find("300"));
        assert_eq!(Some(20), root.find("201").copied());
        assert_eq!(None, root.find("2010").copied());
    }

    #[test]
    fn test_wildcard_2() {
        let mut root = Trie::<u32>::default();

        root.insert("2x1", 20);

        assert_eq!(None, root.find("300"));
        assert_eq!(Some(20), root.find("201").copied());
        assert_eq!(None, root.find("2010").copied());
    }
}
